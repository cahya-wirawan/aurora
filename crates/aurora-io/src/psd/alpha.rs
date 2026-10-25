//! Saved selections as PSD alpha channels (task 156).
//!
//! # What a Photoshop file holds
//!
//! A document's alpha channels are the planes of the **merged image** that
//! follow the colour planes (and the merged image's transparency plane when
//! the layer count is negative), at the file's depth. Three image resources
//! describe them, each listing every such plane in order -- the
//! `Transparency` plane first when there is one, as Photoshop's own files do
//! (`effects/stroke-without-vector-mask.psd`):
//!
//! * `1006`: one Pascal string per plane (the name),
//! * `1045`: the same names as Unicode strings (`u32` length in UTF-16 units
//!   including a trailing NUL, then the units, big-endian),
//! * `1053`: one `u32` identifier per plane (`0` for `Transparency`),
//! * `1077`: `u32` version 1, then 13 bytes per plane: colour space (`u16`),
//!   four colour components (`u16` each), opacity (`u16`, 0..100) and a kind
//!   byte. Photoshop's default alpha channel, which `Select > Save
//!   Selection` makes, is a red overlay at 50 % with kind `1`; the Adobe
//!   specification calls kind `0` "selected, i.e. inverted", `1` "protected"
//!   (masked areas) and `2` a spot colour.
//!
//! # What Aurora does with them
//!
//! **Write.** Each saved selection becomes one plane: white (`255`, or
//! `255 * 257` in the 16-bit file) is selected, black unselected -- the
//! channel `Save Selection` makes, kind `1`. A selection is rasterised over
//! the document, row by row, from [`aurora_doc::Selection::coverage_row_bytes`];
//! nothing is held whole. The first [`MAX_CHANNELS`] selections by name are
//! written, as far as the bytes Aurora would read back stay within its
//! caps ([`read_back_bytes`]); the rest, and any whose region misses the
//! canvas, are itemised. Nothing here has been opened in Photoshop.
//!
//! **Read.** Planes after the colour planes (and the transparency plane) are
//! alpha channels. Kind `1` is read as it is, kind `0` inverted (the
//! specification's "inverted"), kind `2` (spot colour) is left out. A plane
//! becomes a saved [`Selection`]: trimmed to the box of its non-zero texels,
//! a solid box a plain rectangle, anything else a raster mask. All-zero
//! channels hold no selection and are left out. At most 64 selections and
//! 128 MiB of mask are kept; a damaged merged image costs the channels, never
//! the open.

use aurora_core::Rect;
use aurora_doc::{
    MAX_SAVED_SELECTION_BYTES, MAX_SAVED_SELECTIONS, MAX_SELECTION_MASK_PIXELS, Selection,
    SelectionMask, validate_selection_name,
};

use super::{
    ByteSource, Header, IoError, Window, malformed, truncated, try_alloc, undo_prediction,
    unpack_bits,
};

/// The reserved name of the merged image's transparency plane.
pub(crate) const TRANSPARENCY_NAME: &str = "Transparency";

/// Kind byte: the channel stores the selected area inverted (Adobe: "Color
/// selected -- i.e. inverted").
pub(crate) const KIND_INVERTED: u8 = 0;
/// Kind byte: masked areas, Photoshop's default; white is selected.
pub(crate) const KIND_PLAIN: u8 = 1;
/// Kind byte: a spot colour channel.
pub(crate) const KIND_SPOT: u8 = 2;

/// The most alpha channels written (and kept on a read).
pub(crate) const MAX_CHANNELS: usize = MAX_SAVED_SELECTIONS;

/// A resource body larger than this is ignored on a read.
const MAX_RESOURCE_BYTES: usize = 1 << 20;
/// The most entries read from a resource.
const MAX_ENTRIES: usize = 256;
/// The Pascal string limit.
pub(crate) const PASCAL_MAX: usize = 255;

// ---------------------------------------------------------------------
// Write: planning
// ---------------------------------------------------------------------

/// One saved selection chosen to be written as a plane.
#[derive(Debug)]
pub(crate) struct WrittenChannel<'a> {
    /// The name as stored in `1045` (and, ASCII-only, in `1006`).
    pub name: String,
    pub id: u32,
    pub selection: &'a Selection,
}

/// What [`plan`] decided, with the exceptions to itemise.
#[derive(Debug, Default)]
pub(crate) struct Plan<'a> {
    pub channels: Vec<WrittenChannel<'a>>,
    /// Names renamed because Photoshop reserves them: `(was, now)`.
    pub renamed: Vec<(String, String)>,
    /// Names whose region lies wholly outside the canvas.
    pub outside: Vec<String>,
    /// Names not written because Aurora would not read them back within its
    /// caps (or the cap of channels).
    pub over_caps: Vec<String>,
    /// Names whose region extends past the canvas (the part inside is saved).
    pub clipped: Vec<String>,
    /// How many selections were left out because the canvas is larger than
    /// a mask Aurora keeps.
    pub canvas_too_large: usize,
}

/// The bytes of mask Aurora keeps reading `selection` back from a plane over
/// `canvas`: a solid rectangle is read as a rectangle (nothing), an ellipse
/// or a mask as a mask of its box inside the canvas, anything inverted as at
/// most the whole canvas. An upper bound.
pub(crate) fn read_back_bytes(selection: &Selection, canvas: (u32, u32)) -> u64 {
    let area = u64::from(canvas.0) * u64::from(canvas.1);
    if selection.inverted {
        return area;
    }
    if selection.is_rect() {
        return 0;
    }
    clipped(&selection.bounds, canvas).map_or(0, |(w, h)| w * h)
}

/// The size of `bounds` clipped to the canvas, `None` when nothing is left.
fn clipped(bounds: &Rect, canvas: (u32, u32)) -> Option<(u64, u64)> {
    let x0 = bounds.x.max(0);
    let y0 = bounds.y.max(0);
    let x1 = bounds.right().min(i64::from(canvas.0));
    let y1 = bounds.bottom().min(i64::from(canvas.1));
    (x1 > x0 && y1 > y0).then(|| {
        (
            u64::try_from(x1 - x0).unwrap_or(0),
            u64::try_from(y1 - y0).unwrap_or(0),
        )
    })
}

/// Whether the selection's region is wholly outside the canvas (it then
/// covers nothing there). An inverted selection covers the canvas unless its
/// shape fills it, which is not decided here.
fn misses_canvas(selection: &Selection, canvas: (u32, u32)) -> bool {
    if selection.inverted {
        return false;
    }
    clipped(&selection.bounds, canvas).is_none()
}

/// Whether a non-inverted selection's box extends past the canvas.
fn extends_past(selection: &Selection, canvas: (u32, u32)) -> bool {
    if selection.inverted {
        return false;
    }
    let b = &selection.bounds;
    b.x < 0
        || b.y < 0
        || b.right() > i64::from(canvas.0)
        || b.bottom() > i64::from(canvas.1)
}

/// The Pascal bytes of a name: ASCII kept, anything else `?` (the Unicode
/// name in `1045` is the real one), at most [`PASCAL_MAX`] bytes. The flag
/// is whether the name had to be shortened.
pub(crate) fn pascal_bytes(name: &str) -> (Vec<u8>, bool) {
    let mut bytes: Vec<u8> = name
        .chars()
        .map(|c| if c.is_ascii() && !c.is_ascii_control() { c as u8 } else { b'?' })
        .collect();
    let cut = bytes.len() > PASCAL_MAX;
    bytes.truncate(PASCAL_MAX);
    (bytes, cut)
}

/// Chooses which of `saved` (sorted by name) are written over `canvas`.
pub(crate) fn plan<'a>(saved: &[(&str, &'a Selection)], canvas: (u32, u32)) -> Plan<'a> {
    let mut plan = Plan::default();
    let area = u64::from(canvas.0) * u64::from(canvas.1);
    if area > MAX_SELECTION_MASK_PIXELS {
        plan.canvas_too_large = saved.len();
        return plan;
    }
    let mut bytes = 0_u64;
    let mut next_id = 1_u32;
    for &(name, selection) in saved {
        if misses_canvas(selection, canvas) {
            plan.outside.push(name.to_owned());
            continue;
        }
        let cost = read_back_bytes(selection, canvas);
        if plan.channels.len() >= MAX_CHANNELS
            || bytes.saturating_add(cost) > MAX_SAVED_SELECTION_BYTES
        {
            plan.over_caps.push(name.to_owned());
            continue;
        }
        bytes += cost;
        if extends_past(selection, canvas) {
            plan.clipped.push(name.to_owned());
        }
        let mut stored = name.to_owned();
        if stored.eq_ignore_ascii_case(TRANSPARENCY_NAME) {
            stored = format!("{name} 2");
            plan.renamed.push((name.to_owned(), stored.clone()));
        }
        plan.channels.push(WrittenChannel {
            name: stored,
            id: next_id,
            selection,
        });
        next_id += 1;
    }
    plan
}

// ---------------------------------------------------------------------
// Write: resources
// ---------------------------------------------------------------------

/// What the four resources say about one plane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceChannel {
    /// `1006`, already clean ([`pascal_bytes`]).
    pub pascal: Vec<u8>,
    /// `1045`.
    pub name: String,
    /// `1053`.
    pub id: u32,
    /// `1077`: opacity 0..=100.
    pub opacity: u16,
    /// `1077`: the kind byte.
    pub kind: u8,
}

impl ResourceChannel {
    /// The `Transparency` plane Photoshop lists before the alpha channels.
    pub(crate) fn transparency() -> Self {
        Self {
            pascal: TRANSPARENCY_NAME.as_bytes().to_vec(),
            name: TRANSPARENCY_NAME.to_owned(),
            id: 0,
            opacity: 100,
            kind: KIND_PLAIN,
        }
    }

    /// A written alpha channel: Photoshop's default (a red overlay at 50 %,
    /// masked areas).
    pub(crate) fn alpha(name: &str, id: u32) -> Self {
        Self {
            pascal: pascal_bytes(name).0,
            name: name.to_owned(),
            id,
            opacity: 50,
            kind: KIND_PLAIN,
        }
    }
}

/// The resource blocks `(id, body)` for `channels` (the `Transparency` plane
/// first), ascending by id. Empty for no channels.
pub(crate) fn resource_blocks(channels: &[ResourceChannel]) -> Vec<(u16, Vec<u8>)> {
    if channels.is_empty() {
        return Vec::new();
    }
    let mut names = Vec::new();
    let mut unicode = Vec::new();
    let mut ids = Vec::new();
    let mut display = 1_u32.to_be_bytes().to_vec();
    for c in channels {
        names.push(u8::try_from(c.pascal.len()).unwrap_or(u8::MAX));
        names.extend_from_slice(&c.pascal);
        let units: Vec<u16> = c.name.encode_utf16().collect();
        let count = u32::try_from(units.len() + 1).unwrap_or(u32::MAX);
        unicode.extend_from_slice(&count.to_be_bytes());
        for unit in units {
            unicode.extend_from_slice(&unit.to_be_bytes());
        }
        unicode.extend_from_slice(&[0, 0]);
        ids.extend_from_slice(&c.id.to_be_bytes());
        // Colour space 0 (RGB), red, then the opacity and the kind.
        display.extend_from_slice(&0_u16.to_be_bytes());
        display.extend_from_slice(&0xffff_u16.to_be_bytes());
        display.extend_from_slice(&[0; 6]);
        display.extend_from_slice(&c.opacity.to_be_bytes());
        display.push(c.kind);
    }
    vec![(1006, names), (1045, unicode), (1053, ids), (1077, display)]
}

// ---------------------------------------------------------------------
// Read: resources
// ---------------------------------------------------------------------

/// The raw bodies of the alpha resources a file carries.
#[derive(Debug, Default)]
pub(crate) struct ResourceBodies {
    pascal: Option<Vec<u8>>,
    unicode: Option<Vec<u8>>,
    display: Option<Vec<u8>>,
}

impl ResourceBodies {
    /// Keeps the body of resource `id` when it is one of ours and small.
    pub(crate) fn offer(&mut self, id: u16, body: &[u8]) {
        if body.len() > MAX_RESOURCE_BYTES {
            return;
        }
        let slot = match id {
            1006 => &mut self.pascal,
            1045 => &mut self.unicode,
            1077 => &mut self.display,
            _ => return,
        };
        if slot.is_none() {
            *slot = Some(body.to_vec());
        }
    }

    fn pascal_names(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut rest: &[u8] = self.pascal.as_deref().unwrap_or_default();
        while let Some((&len, tail)) = rest.split_first() {
            if out.len() >= MAX_ENTRIES {
                break;
            }
            let len = usize::from(len);
            let Some(text) = tail.get(..len) else { break };
            out.push(text.to_vec());
            rest = tail.get(len..).unwrap_or_default();
        }
        out
    }

    fn unicode_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest: &[u8] = self.unicode.as_deref().unwrap_or_default();
        while out.len() < MAX_ENTRIES {
            let Some((count, tail)) = rest.split_at_checked(4) else {
                break;
            };
            let count = count
                .try_into()
                .map(u32::from_be_bytes)
                .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
            let Some(bytes) = count.checked_mul(2).and_then(|n| tail.get(..n)) else {
                break;
            };
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| u16::from_be_bytes([p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0)]))
                .collect();
            let trimmed = units
                .iter()
                .rposition(|&u| u != 0)
                .map_or(&units[..0], |last| units.get(..=last).unwrap_or(&[]));
            out.push(String::from_utf16_lossy(trimmed));
            rest = tail.get(bytes.len()..).unwrap_or_default();
        }
        out
    }

    /// `(opacity, kind)` per listed plane.
    fn display_entries(&self) -> Vec<(u16, u8)> {
        let body: &[u8] = self.display.as_deref().unwrap_or_default();
        body.get(4..)
            .unwrap_or_default()
            .chunks_exact(13)
            .take(MAX_ENTRIES)
            .map(|e| {
                let opacity = u16::from_be_bytes([e.get(10).copied().unwrap_or(0), e.get(11).copied().unwrap_or(0)]);
                (opacity, e.get(12).copied().unwrap_or(KIND_PLAIN))
            })
            .collect()
    }

    /// The name listed for plane `index` of the non-colour planes: the
    /// Unicode one when present, else the Pascal one (bytes read as
    /// Latin-1).
    fn name_of(&self, index: usize, pascal: &[Vec<u8>], unicode: &[String]) -> Option<String> {
        let _ = self;
        unicode.get(index).cloned().or_else(|| {
            pascal
                .get(index)
                .map(|bytes| bytes.iter().map(|&b| char::from(b)).collect())
        })
    }

    /// The first listed name, for telling an untagged `Transparency` plane.
    pub(crate) fn first_name_is_transparency(&self) -> bool {
        let unicode = self.unicode_names();
        let pascal = self.pascal_names();
        self.name_of(0, &pascal, &unicode)
            .is_some_and(|n| n == TRANSPARENCY_NAME)
    }
}

// ---------------------------------------------------------------------
// Read: planes
// ---------------------------------------------------------------------

/// What reading the alpha planes gave.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The saved selections, in file order, with their (unique, valid) names.
    pub selections: Vec<(String, Selection)>,
    /// Spot colour channels, left out.
    pub spot: u64,
    /// Channels holding no selection (all zero).
    pub empty: u64,
    /// Channels past the caps.
    pub over_caps: u64,
    /// Channels in a merged image that could not be read.
    pub unreadable: u64,
}

/// A name that is valid, unique and at most 64 characters: the file's own
/// when it passes [`validate_selection_name`], else `Alpha N`.
fn selection_name(raw: Option<String>, ordinal: usize, taken: &[(String, Selection)]) -> String {
    let fallback = format!("Alpha {ordinal}");
    let base = raw
        .and_then(|raw| {
            validate_selection_name(&raw)
                .ok()
                .or_else(|| validate_selection_name(&aurora_doc::sanitize_display_name(&raw)).ok())
        })
        .unwrap_or(fallback);
    let is_taken = |name: &str| taken.iter().any(|(n, _)| n == name);
    if !is_taken(&base) {
        return base;
    }
    let mut n = 2_u32;
    loop {
        let suffix = format!(" {n}");
        let keep = aurora_doc::SELECTION_NAME_MAX_CHARS.saturating_sub(suffix.chars().count());
        let head: String = base.chars().take(keep).collect();
        let candidate = format!("{}{suffix}", head.trim_end());
        if !is_taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// One plane's rows turned into coverage bytes (`0..=255`), the file's
/// depth folded to 8 bits.
fn coverage_row(row: &[u8], bps: usize, out: &mut Vec<u8>) {
    match bps {
        2 => out.extend(row.chunks_exact(2).map(|p| {
            let v = u32::from(u16::from_be_bytes([
                p.first().copied().unwrap_or(0),
                p.get(1).copied().unwrap_or(0),
            ]));
            u8::try_from((v * 255 + 32_767) / 65_535).unwrap_or(u8::MAX)
        })),
        4 => out.extend(row.chunks_exact(4).map(|p| {
            let v = f32::from_be_bytes([
                p.first().copied().unwrap_or(0),
                p.get(1).copied().unwrap_or(0),
                p.get(2).copied().unwrap_or(0),
                p.get(3).copied().unwrap_or(0),
            ]);
            // Clamped to 0..=255 before the cast; NaN reads as 0.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = if v.is_nan() { 0 } else { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
            byte
        })),
        _ => out.extend_from_slice(row),
    }
}

/// A selection from a full-canvas coverage plane, trimmed. `None` when the
/// plane is all zero.
fn selection_of(plane: &[u8], width: u32, height: u32) -> Result<Option<Selection>, IoError> {
    let w = width as usize;
    let mut top = None;
    let mut bottom = 0;
    let (mut left, mut right) = (usize::MAX, 0);
    for (y, row) in plane.chunks_exact(w.max(1)).enumerate() {
        let (Some(first), Some(last)) = (
            row.iter().position(|&b| b != 0),
            row.iter().rposition(|&b| b != 0),
        ) else {
            continue;
        };
        top.get_or_insert(y);
        bottom = y;
        left = left.min(first);
        right = right.max(last);
    }
    let Some(top) = top else { return Ok(None) };
    let _ = height;
    let bounds = Rect {
        x: i64::try_from(left).unwrap_or(0),
        y: i64::try_from(top).unwrap_or(0),
        width: u32::try_from(right - left + 1).unwrap_or(0),
        height: u32::try_from(bottom - top + 1).unwrap_or(0),
    };
    let box_w = right - left + 1;
    let mut data: Vec<u8> = try_alloc(box_w * (bottom - top + 1))?;
    let mut solid = true;
    for row in plane.chunks_exact(w.max(1)).skip(top).take(bottom - top + 1) {
        let part = row.get(left..=right).unwrap_or_default();
        solid &= part.iter().all(|&b| b == u8::MAX);
        data.extend_from_slice(part);
    }
    if solid {
        return Ok(Some(Selection::new(bounds)));
    }
    let mask = SelectionMask::new(bounds, data).map_err(|_| malformed("an alpha channel mask"))?;
    Ok(Some(Selection::from_mask(std::sync::Arc::new(mask))))
}

/// `Read` over the rest of a window, for the ZIP-compressed merged image.
struct SourceRead<'a> {
    src: &'a mut dyn ByteSource,
    pos: u64,
    end: u64,
}

impl std::io::Read for SourceRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        const BLOCK: usize = 64 * 1024;
        let left = usize::try_from(self.end.saturating_sub(self.pos)).unwrap_or(usize::MAX);
        let n = buf.len().min(BLOCK).min(left);
        if n == 0 {
            return Ok(0);
        }
        let bytes = self
            .src
            .read_at(self.pos, n, "merged image data")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.pos += n as u64;
        let dst = buf.get_mut(..n).ok_or(std::io::ErrorKind::InvalidInput)?;
        dst.copy_from_slice(&bytes);
        Ok(n)
    }
}

/// How many bytes one read of raw rows asks for at most.
const RAW_BLOCK_BYTES: usize = 1 << 20;

/// Walks the merged image's planes, calling `on_row(i, row)` for every row
/// of `wanted[i]` (absolute plane numbers, ascending), in order. Nothing
/// larger than a block of rows is held. Any compression the reader knows.
#[allow(clippy::too_many_lines)] // one arm per compression, as the merged image's own walk
fn walk_planes(
    src: &mut dyn ByteSource,
    mut window: Window,
    header: Header,
    wanted: &[usize],
    on_row: &mut dyn FnMut(usize, &[u8]) -> Result<(), IoError>,
) -> Result<(), IoError> {
    let size = || malformed("merged image size");
    let width = header.width as usize;
    let height = header.height as usize;
    let row_bytes = header.row_bytes(width).ok_or_else(size)?;
    let plane_len = row_bytes.checked_mul(height).ok_or_else(size)?;
    let channels = usize::from(header.channels);
    let Some(&last) = wanted.last() else {
        return Ok(());
    };
    if last >= channels || row_bytes == 0 {
        return Err(malformed("an alpha channel the file does not hold"));
    }
    let compression = u16::from_be_bytes(window.take_array(src, "merged image compression")?);
    match compression {
        0 => {
            let needed = plane_len
                .checked_mul(last + 1)
                .ok_or_else(size)?;
            if !window.fits(needed) {
                return Err(truncated("merged image data"));
            }
            let block_rows = (RAW_BLOCK_BYTES / row_bytes).max(1);
            for (i, &plane) in wanted.iter().enumerate() {
                let mut offset = window.pos + (plane * plane_len) as u64;
                let mut y = 0;
                while y < height {
                    let rows = block_rows.min(height - y);
                    let bytes = src.read_at(offset, rows * row_bytes, "merged image data")?;
                    for row in bytes.chunks_exact(row_bytes) {
                        on_row(i, row)?;
                    }
                    offset += (rows * row_bytes) as u64;
                    y += rows;
                }
            }
        }
        1 => {
            let entry = if header.psb() { 4 } else { 2 };
            let table_all = channels
                .checked_mul(height)
                .and_then(|n| n.checked_mul(entry))
                .ok_or_else(size)?;
            let used_rows = (last + 1).checked_mul(height).ok_or_else(size)?;
            if !window.fits(table_all) {
                return Err(truncated("merged image RLE row lengths"));
            }
            let table = src.read_at(window.pos, used_rows * entry, "merged image RLE row lengths")?;
            let lengths: Vec<usize> = table
                .chunks_exact(entry)
                .map(|c| match c {
                    [a, b] => usize::from(u16::from_be_bytes([*a, *b])),
                    [a, b, c, d] => u32::from_be_bytes([*a, *b, *c, *d]) as usize,
                    _ => 0,
                })
                .collect();
            drop(table);
            let mut offset = window.pos + table_all as u64;
            let mut offsets = Vec::with_capacity(lengths.len());
            for len in &lengths {
                offsets.push(offset);
                offset = offset.saturating_add(*len as u64);
            }
            if offset > window.end {
                return Err(truncated("merged image RLE rows"));
            }
            let mut row: Vec<u8> = Vec::with_capacity(row_bytes);
            for (i, &plane) in wanted.iter().enumerate() {
                for y in 0..height {
                    let k = plane * height + y;
                    let (Some(&at), Some(&len)) = (offsets.get(k), lengths.get(k)) else {
                        return Err(truncated("merged image RLE rows"));
                    };
                    let packed = src.read_at(at, len, "merged image RLE row")?;
                    row.clear();
                    unpack_bits(&packed, &mut row, row_bytes)?;
                    on_row(i, &row)?;
                }
            }
        }
        2 | 3 => {
            let reader = SourceRead {
                src,
                pos: window.pos,
                end: window.end,
            };
            let mut decoder = flate2::read::ZlibDecoder::new(reader);
            let mut row = vec![0_u8; row_bytes];
            let mut next = 0;
            for plane in 0..=last {
                for _ in 0..height {
                    std::io::Read::read_exact(&mut decoder, &mut row)
                        .map_err(|_| malformed("ZIP-compressed merged image"))?;
                    if compression == 3 {
                        undo_prediction(&mut row, row_bytes, header.bytes_per_sample());
                    }
                    if wanted.get(next) == Some(&plane) {
                        on_row(next, &row)?;
                    }
                }
                if wanted.get(next) == Some(&plane) {
                    next += 1;
                }
            }
        }
        other => return Err(IoError::UnsupportedPsdCompression(other)),
    }
    Ok(())
}

/// Reads the file's alpha planes as saved selections. `color_planes` is the
/// number of colour planes and `first_plane` the first plane that may be an
/// alpha channel (after the merged transparency plane, when there is one).
/// The merged image is `merged`, the rest of the file after the layer and
/// mask section. Never fails for damage in the merged image -- those
/// channels are counted in [`Outcome::unreadable`] -- only for an I/O error.
pub(crate) fn read_channels(
    src: &mut dyn ByteSource,
    merged: Window,
    header: Header,
    color_planes: usize,
    first_plane: usize,
    resources: &ResourceBodies,
) -> Result<Outcome, IoError> {
    let mut outcome = Outcome::default();
    let total = usize::from(header.channels);
    if first_plane >= total {
        return Ok(outcome);
    }
    let pascal = resources.pascal_names();
    let unicode = resources.unicode_names();
    let display = resources.display_entries();
    // (plane, kind, listed name), spot channels set aside.
    let mut wanted: Vec<(usize, u8, Option<String>)> = Vec::new();
    for plane in first_plane..total {
        let index = plane - color_planes;
        let kind = display.get(index).map_or(KIND_PLAIN, |&(_, kind)| kind);
        if kind == KIND_SPOT {
            outcome.spot += 1;
            continue;
        }
        wanted.push((plane, kind, resources.name_of(index, &pascal, &unicode)));
    }
    let (width, height) = (header.width, header.height);
    let area = u64::from(width) * u64::from(height);
    if area > MAX_SELECTION_MASK_PIXELS {
        outcome.over_caps += wanted.len() as u64;
        return Ok(outcome);
    }
    let planes: Vec<usize> = wanted.iter().map(|(plane, ..)| *plane).collect();
    let bps = header.bytes_per_sample();
    let rows_per_plane = height as usize;
    let mut current: Vec<u8> = Vec::new();
    let mut rows_done = 0_usize;
    let mut bytes = 0_u64;
    let mut finished = 0_usize;
    let result = walk_planes(src, merged, header, &planes, &mut |i, row| {
        let Some((_, kind, name)) = wanted.get(i) else {
            return Ok(());
        };
        if rows_done == 0 {
            current = try_alloc(area as usize)?;
        }
        let from = current.len();
        coverage_row(row, bps, &mut current);
        if *kind == KIND_INVERTED {
            for b in current.iter_mut().skip(from) {
                *b = u8::MAX - *b;
            }
        }
        rows_done += 1;
        if rows_done < rows_per_plane {
            return Ok(());
        }
        rows_done = 0;
        finished = i + 1;
        let plane = std::mem::take(&mut current);
        match selection_of(&plane, width, height)? {
            None => outcome.empty += 1,
            Some(selection) => {
                bytes = bytes.saturating_add(selection.mask_bytes());
                if outcome.selections.len() >= MAX_SAVED_SELECTIONS
                    || bytes > MAX_SAVED_SELECTION_BYTES
                {
                    outcome.over_caps += 1;
                } else {
                    let ordinal = outcome.selections.len() + outcome.empty as usize + 1;
                    let name = selection_name(name.clone(), ordinal, &outcome.selections);
                    outcome.selections.push((name, selection));
                }
            }
        }
        Ok(())
    });
    match result {
        Ok(()) => {}
        Err(err @ IoError::Io(_)) => return Err(err),
        Err(_) => outcome.unreadable += wanted.len().saturating_sub(finished) as u64,
    }
    Ok(outcome)
}
