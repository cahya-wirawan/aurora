//! A small PSD/PSB **writer**, test-only: it exists so the reader's tests
//! can build exactly the file they need — any version, depth,
//! compression, block layout or deliberate defect — instead of depending
//! on a binary fixture for every case. Its record layout follows
//! `spike/psd-write/src/psd.rs` (whose output Photoshop and psd-tools
//! both open), extended with PSB, 16-bit, `Lr16`, all four compressions,
//! masks and arbitrary tagged blocks. It is never compiled outside
//! `cfg(test)` and makes no claim to write files Photoshop accepts in
//! every combination.

use std::io::Write as _;

/// One layer record, bottom-to-top order in [`TestPsd::layers`].
#[derive(Clone, Debug)]
pub(crate) struct TestLayer {
    /// The legacy Pascal name.
    pub name: String,
    /// A `luni` block's name, or none at all.
    pub luni: Option<String>,
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    /// `(channel id, decoded plane)` — the plane is big-endian samples at
    /// the file's depth, `width * height * bps` bytes.
    pub channels: Vec<(i16, Vec<u8>)>,
    pub compression: u16,
    pub blend: [u8; 4],
    pub opacity: u8,
    pub clipping: u8,
    pub flags: u8,
    /// An `lsct` block: divider type and optional blend key.
    pub section: Option<(u32, Option<[u8; 4]>)>,
    /// An `iOpa` block.
    pub fill: Option<u8>,
    /// Extra tagged blocks, written as given.
    pub blocks: Vec<([u8; 4], Vec<u8>)>,
    /// Mask data: `(top, left, bottom, right, default colour, flags)`.
    pub mask: Option<(i32, i32, i32, i32, u8, u8)>,
}

impl TestLayer {
    /// A pixel layer at `(left, top)` from straight RGBA samples (one
    /// `[r, g, b, a]` per pixel, each `u16` — only the low byte is used at
    /// 8-bit depth), with a transparency channel.
    pub fn pixels(
        name: &str,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
        depth: u16,
        rgba: &[[u16; 4]],
    ) -> Self {
        let plane = |c: usize| -> Vec<u8> {
            rgba.iter()
                .flat_map(|px| {
                    let v = px.get(c).copied().unwrap_or(0);
                    if depth == 16 {
                        v.to_be_bytes().to_vec()
                    } else {
                        vec![v.to_be_bytes().get(1).copied().unwrap_or(0)]
                    }
                })
                .collect()
        };
        Self {
            name: name.to_owned(),
            luni: Some(name.to_owned()),
            top,
            left,
            bottom: top + height,
            right: left + width,
            channels: vec![(-1, plane(3)), (0, plane(0)), (1, plane(1)), (2, plane(2))],
            compression: 0,
            blend: *b"norm",
            opacity: 255,
            clipping: 0,
            flags: 0,
            section: None,
            fill: None,
            blocks: Vec::new(),
            mask: None,
        }
    }

    /// An empty-rectangle record with the four usual zero-size channels.
    pub fn empty(name: &str) -> Self {
        Self {
            channels: vec![
                (-1, Vec::new()),
                (0, Vec::new()),
                (1, Vec::new()),
                (2, Vec::new()),
            ],
            ..Self::pixels(name, 0, 0, 0, 0, 8, &[])
        }
    }

    /// The bounding "end of group" divider (type 3), below its children.
    pub fn divider() -> Self {
        Self {
            section: Some((3, None)),
            ..Self::empty("</Layer group>")
        }
    }

    /// A group header (type 1, open folder), above its children.
    pub fn group(name: &str, blend: [u8; 4]) -> Self {
        Self {
            blend,
            section: Some((1, Some(blend))),
            ..Self::empty(name)
        }
    }

    pub fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    fn width(&self) -> usize {
        usize::try_from(self.right - self.left).unwrap_or(0)
    }

    fn height(&self) -> usize {
        usize::try_from(self.bottom - self.top).unwrap_or(0)
    }
}

/// A whole file.
#[derive(Clone, Debug)]
pub(crate) struct TestPsd {
    pub version: u16,
    pub width: u32,
    pub height: u32,
    pub depth: u16,
    pub color_mode: u16,
    pub channels: u16,
    pub layers: Vec<TestLayer>,
    /// Put the layer info in an `Lr16` global block (what 16-bit
    /// Photoshop files do) instead of the layer-info section.
    pub lr16: bool,
    pub negative_count: bool,
    pub resources: Vec<u8>,
    /// The merged image's planes (one per header channel); white when
    /// `None`.
    pub merged: Option<Vec<Vec<u8>>>,
    pub merged_compression: u16,
}

impl TestPsd {
    pub fn new(version: u16, width: u32, height: u32, depth: u16) -> Self {
        Self {
            version,
            width,
            height,
            depth,
            color_mode: 3,
            channels: 3,
            layers: Vec::new(),
            lr16: false,
            negative_count: false,
            resources: Vec::new(),
            merged: None,
            merged_compression: 0,
        }
    }

    pub fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    fn psb(&self) -> bool {
        self.version == 2
    }

    fn bps(&self) -> usize {
        if self.depth == 16 { 2 } else { 1 }
    }

    pub fn write(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(b"8BPS");
        o.extend_from_slice(&self.version.to_be_bytes());
        o.extend_from_slice(&[0; 6]);
        o.extend_from_slice(&self.channels.to_be_bytes());
        o.extend_from_slice(&self.height.to_be_bytes());
        o.extend_from_slice(&self.width.to_be_bytes());
        o.extend_from_slice(&self.depth.to_be_bytes());
        o.extend_from_slice(&self.color_mode.to_be_bytes());
        o.extend_from_slice(&0u32.to_be_bytes());
        put_u32(&mut o, self.resources.len());
        o.extend_from_slice(&self.resources);

        let body = self.layer_info_body();
        let mut section = Vec::new();
        if self.lr16 {
            put_len(&mut section, 0, self.psb());
            put_u32(&mut section, 0); // global layer mask info
            section.extend_from_slice(b"8BIM");
            section.extend_from_slice(b"Lr16");
            put_len(&mut section, body.len(), self.psb());
            section.extend_from_slice(&body);
            while !section.len().is_multiple_of(4) {
                section.push(0);
            }
        } else {
            let mut body = body;
            if !body.len().is_multiple_of(2) {
                body.push(0);
            }
            put_len(&mut section, body.len(), self.psb());
            section.extend_from_slice(&body);
            put_u32(&mut section, 0);
        }
        put_len(&mut o, section.len(), self.psb());
        o.extend_from_slice(&section);

        o.extend_from_slice(&self.merged_bytes());
        o
    }

    fn layer_info_body(&self) -> Vec<u8> {
        if self.layers.is_empty() {
            return Vec::new();
        }
        let psb = self.psb();
        let mut body = Vec::new();
        let count = i16::try_from(self.layers.len()).unwrap_or(i16::MAX);
        let count = if self.negative_count { -count } else { count };
        body.extend_from_slice(&count.to_be_bytes());
        let mut blobs = Vec::new();
        for layer in &self.layers {
            let (layer_w, layer_h) = (layer.width(), layer.height());
            let mut layer_blobs = Vec::new();
            for (id, plane) in &layer.channels {
                let (cw, ch) = match (id, layer.mask) {
                    (-2, Some((m_top, m_left, m_bottom, m_right, _, _))) => (
                        usize::try_from(m_right - m_left).unwrap_or(0),
                        usize::try_from(m_bottom - m_top).unwrap_or(0),
                    ),
                    _ => (layer_w, layer_h),
                };
                let blob = encode_channel(plane, cw, ch, self.bps(), layer.compression, psb);
                layer_blobs.push((*id, blob));
            }
            body.extend_from_slice(&layer.top.to_be_bytes());
            body.extend_from_slice(&layer.left.to_be_bytes());
            body.extend_from_slice(&layer.bottom.to_be_bytes());
            body.extend_from_slice(&layer.right.to_be_bytes());
            body.extend_from_slice(
                &u16::try_from(layer_blobs.len())
                    .unwrap_or(u16::MAX)
                    .to_be_bytes(),
            );
            for (id, blob) in &layer_blobs {
                body.extend_from_slice(&id.to_be_bytes());
                put_len(&mut body, blob.len(), psb);
            }
            body.extend_from_slice(b"8BIM");
            body.extend_from_slice(&layer.blend);
            body.extend_from_slice(&[layer.opacity, layer.clipping, layer.flags, 0]);
            let extra = record_extra(layer, psb);
            put_u32(&mut body, extra.len());
            body.extend_from_slice(&extra);
            blobs.push(layer_blobs);
        }
        for layer_blobs in blobs {
            for (_, blob) in layer_blobs {
                body.extend_from_slice(&blob);
            }
        }
        body
    }

    fn merged_bytes(&self) -> Vec<u8> {
        let bps = self.bps();
        let (w, h) = (self.width as usize, self.height as usize);
        let planes = self.merged.clone().unwrap_or_else(|| {
            (0..self.channels)
                .map(|_| vec![0xFF; w * h * bps])
                .collect()
        });
        let mut o = Vec::new();
        o.extend_from_slice(&self.merged_compression.to_be_bytes());
        match self.merged_compression {
            1 => {
                let mut counts = Vec::new();
                let mut data = Vec::new();
                for plane in &planes {
                    for row in plane.chunks(w * bps) {
                        let packed = pack_bits(row);
                        put_count(&mut counts, packed.len(), self.psb());
                        data.extend_from_slice(&packed);
                    }
                }
                o.extend_from_slice(&counts);
                o.extend_from_slice(&data);
            }
            2 | 3 => {
                let mut all: Vec<u8> = planes.concat();
                if self.merged_compression == 3 {
                    predict(&mut all, w * bps, bps);
                }
                o.extend_from_slice(&zlib(&all));
            }
            _ => {
                for plane in &planes {
                    o.extend_from_slice(plane);
                }
            }
        }
        o
    }
}

fn record_extra(layer: &TestLayer, psb: bool) -> Vec<u8> {
    let mut extra = Vec::new();
    match layer.mask {
        Some((t, l, b, r, default, flags)) => {
            put_u32(&mut extra, 20);
            for v in [t, l, b, r] {
                extra.extend_from_slice(&v.to_be_bytes());
            }
            extra.extend_from_slice(&[default, flags, 0, 0]);
        }
        None => put_u32(&mut extra, 0),
    }
    put_u32(&mut extra, 0); // blending ranges
    let name = layer.name.as_bytes();
    let len = name.len().min(255);
    let name_start = extra.len();
    extra.push(u8::try_from(len).unwrap_or(255));
    extra.extend_from_slice(name.get(..len).unwrap_or(&[]));
    while !(extra.len() - name_start).is_multiple_of(4) {
        extra.push(0);
    }
    if let Some(unicode) = &layer.luni {
        let units: Vec<u16> = unicode.encode_utf16().collect();
        let mut data = Vec::new();
        put_u32(&mut data, units.len());
        for unit in units {
            data.extend_from_slice(&unit.to_be_bytes());
        }
        put_block(&mut extra, *b"luni", &data, psb);
    }
    if let Some((kind, blend)) = layer.section {
        let mut data = kind.to_be_bytes().to_vec();
        if let Some(key) = blend {
            data.extend_from_slice(b"8BIM");
            data.extend_from_slice(&key);
        }
        put_block(&mut extra, *b"lsct", &data, psb);
    }
    if let Some(fill) = layer.fill {
        put_block(&mut extra, *b"iOpa", &[fill, 0, 0, 0], psb);
    }
    for (key, data) in &layer.blocks {
        put_block(&mut extra, *key, data, psb);
    }
    extra
}

pub(crate) fn put_block(out: &mut Vec<u8>, key: [u8; 4], data: &[u8], psb: bool) {
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(&key);
    let wide = psb && super::PSB_WIDE_KEYS.contains(&&key);
    put_len(out, data.len(), wide);
    out.extend_from_slice(data);
}

fn put_u32(out: &mut Vec<u8>, v: usize) {
    out.extend_from_slice(&u32::try_from(v).unwrap_or(u32::MAX).to_be_bytes());
}

fn put_len(out: &mut Vec<u8>, v: usize, wide: bool) {
    if wide {
        out.extend_from_slice(&(v as u64).to_be_bytes());
    } else {
        put_u32(out, v);
    }
}

fn put_count(out: &mut Vec<u8>, v: usize, psb: bool) {
    if psb {
        put_u32(out, v);
    } else {
        out.extend_from_slice(&u16::try_from(v).unwrap_or(u16::MAX).to_be_bytes());
    }
}

/// One channel with its compression field, encoded the way `compression`
/// says.
pub(crate) fn encode_channel(
    plane: &[u8],
    width: usize,
    height: usize,
    bps: usize,
    compression: u16,
    psb: bool,
) -> Vec<u8> {
    let mut out = compression.to_be_bytes().to_vec();
    let row = width * bps;
    match compression {
        1 => {
            let mut counts = Vec::new();
            let mut data = Vec::new();
            if row > 0 {
                for r in plane.chunks(row).take(height) {
                    let packed = pack_bits(r);
                    put_count(&mut counts, packed.len(), psb);
                    data.extend_from_slice(&packed);
                }
            }
            out.extend_from_slice(&counts);
            out.extend_from_slice(&data);
        }
        2 => out.extend_from_slice(&zlib(plane)),
        3 => {
            let mut predicted = plane.to_vec();
            predict(&mut predicted, row, bps);
            out.extend_from_slice(&zlib(&predicted));
        }
        _ => out.extend_from_slice(plane),
    }
    out
}

pub(crate) fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    if encoder.write_all(data).is_err() {
        return Vec::new();
    }
    encoder.finish().unwrap_or_default()
}

/// The inverse of the reader's `undo_prediction`.
fn predict(data: &mut [u8], row_bytes: usize, bps: usize) {
    if row_bytes == 0 {
        return;
    }
    for row in data.chunks_mut(row_bytes) {
        if bps == 2 {
            let mut previous: u16 = 0;
            for pair in row.chunks_exact_mut(2) {
                if let [hi, lo] = pair {
                    let value = u16::from_be_bytes([*hi, *lo]);
                    [*hi, *lo] = value.wrapping_sub(previous).to_be_bytes();
                    previous = value;
                }
            }
        } else {
            let mut previous: u8 = 0;
            for byte in row.iter_mut() {
                let value = *byte;
                *byte = value.wrapping_sub(previous);
                previous = value;
            }
        }
    }
}

/// `PackBits`: runs of two or more equal bytes as repeats, everything else
/// as literals, both capped at 128.
pub(crate) fn pack_bits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = row;
    while let Some(&first) = rest.first() {
        let run = rest.iter().take(128).take_while(|&&b| b == first).count();
        if run >= 2 {
            out.push(u8::try_from(257 - run).unwrap_or(0));
            out.push(first);
            rest = rest.get(run..).unwrap_or(&[]);
        } else {
            let mut n = 1;
            while n < 128 {
                match (rest.get(n), rest.get(n + 1)) {
                    (Some(a), Some(b)) if a == b => break,
                    (Some(_), _) => n += 1,
                    (None, _) => break,
                }
            }
            out.push(u8::try_from(n - 1).unwrap_or(0));
            out.extend_from_slice(rest.get(..n).unwrap_or(&[]));
            rest = rest.get(n..).unwrap_or(&[]);
        }
    }
    out
}
