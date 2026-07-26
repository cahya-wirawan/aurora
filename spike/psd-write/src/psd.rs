//! A minimal PSD writer, built from the format specification.
//!
//! Scope is deliberately narrow: 8-bit RGB, uncompressed channel data, flat
//! layer list with names, opacity, blend modes, visibility, and per-layer alpha.
//! No groups, masks, text, smart objects, or effects — those are the *hard*
//! parts and the point of this spike is to find out whether the easy parts work
//! at all before Phase 3 commits to the rest.
//!
//! Everything here is big-endian. Every length field covers a precisely defined
//! span, and getting one wrong produces a file that readers reject with no
//! useful diagnostic — which is the main thing this spike is measuring.

pub struct Layer {
    pub name: String,
    /// Left, top, right, bottom in document space. Layers may be smaller than
    /// the canvas; this is how PSD keeps them sparse.
    pub rect: (i32, i32, i32, i32),
    /// RGBA8, row-major, `rect` sized.
    pub pixels: Vec<u8>,
    pub opacity: u8,
    pub visible: bool,
    /// Four-character blend key, e.g. `norm`, `mul `, `scrn`.
    pub blend: [u8; 4],
}

impl Layer {
    pub fn width(&self) -> u32 {
        (self.rect.2 - self.rect.0).max(0) as u32
    }
    pub fn height(&self) -> u32 {
        (self.rect.3 - self.rect.1).max(0) as u32
    }
    fn plane(&self, channel: usize) -> Vec<u8> {
        // PSD stores channels planar; Aurora holds them interleaved.
        self.pixels.iter().skip(channel).step_by(4).copied().collect()
    }
}

pub struct Document {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
}

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn raw(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }
    /// Pascal string padded so the *whole field* is a multiple of `align`.
    fn pascal(&mut self, s: &str, align: usize) {
        let bytes = s.as_bytes();
        let len = bytes.len().min(255);
        self.u8(len as u8);
        self.raw(&bytes[..len]);
        let written = 1 + len;
        let pad = (align - (written % align)) % align;
        self.0.extend(std::iter::repeat_n(0u8, pad));
    }
}

impl Document {
    /// Flatten for the composite image section.
    ///
    /// Every PSD carries a flattened copy so non-Photoshop readers have
    /// something to show. Getting this wrong is invisible in Photoshop and
    /// obvious everywhere else — a trap worth knowing about.
    fn composite_planar(&self) -> Vec<u8> {
        let n = (self.width * self.height) as usize;
        // White background, matching what a viewer expects for a flattened PSD.
        let mut out = vec![255u8; n * 3];
        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            let (lw, lh) = (layer.width() as i32, layer.height() as i32);
            let layer_alpha = f32::from(layer.opacity) / 255.0;
            for y in 0..lh {
                for x in 0..lw {
                    let dx = layer.rect.0 + x;
                    let dy = layer.rect.1 + y;
                    if dx < 0 || dy < 0 || dx >= self.width as i32 || dy >= self.height as i32 {
                        continue;
                    }
                    let si = ((y * lw + x) as usize) * 4;
                    let a = (f32::from(layer.pixels[si + 3]) / 255.0) * layer_alpha;
                    if a <= 0.0 {
                        continue;
                    }
                    let di = (dy as usize * self.width as usize + dx as usize) * 3;
                    for c in 0..3 {
                        let s = f32::from(layer.pixels[si + c]) / 255.0;
                        let d = f32::from(out[di + c]) / 255.0;
                        // The stored preview must apply blend modes. Compositing
                        // everything as `normal` produces a file that Photoshop
                        // displays correctly (it recomputes from layer data) but
                        // that every other viewer shows differently — measured at
                        // 44 % of pixels wrong before this was fixed.
                        let blended = match &layer.blend {
                            b"mul " => s * d,
                            b"scrn" => 1.0 - (1.0 - s) * (1.0 - d),
                            _ => s,
                        };
                        out[di + c] = ((blended * a + d * (1.0 - a)) * 255.0).round() as u8;
                    }
                }
            }
        }
        // PSD stores the composite planar — all red, then all green, then all
        // blue. Writing it interleaved produces a file both readers accept and
        // display as a fine dither, which is a memorable way to learn that
        // "the reader opened it" is not the same as "the reader read it right".
        let mut planar = Vec::with_capacity(out.len());
        for c in 0..3 {
            planar.extend(out.iter().skip(c).step_by(3).copied());
        }
        planar
    }

    pub fn write(&self) -> Vec<u8> {
        let mut o = Out::default();

        // --- File header -----------------------------------------------------
        o.raw(b"8BPS");
        o.u16(1); // version 1 = PSD (2 would be PSB)
        o.raw(&[0; 6]); // reserved
        o.u16(3); // channels in the composite
        o.u32(self.height);
        o.u32(self.width);
        o.u16(8); // bits per channel
        o.u16(3); // colour mode: RGB

        o.u32(0); // colour mode data: none
        o.u32(0); // image resources: none

        // --- Layer and mask information --------------------------------------
        // Built into a buffer first because the section length prefixes content
        // whose size is not known until it is serialized.
        let mut layer_info = Out::default();
        layer_info.i16(self.layers.len() as i16);

        // Channel data is emitted after *all* layer records, but each record
        // declares its channel byte counts, so it has to be computed up front.
        let mut channel_blobs: Vec<Vec<Vec<u8>>> = Vec::new();
        for layer in &self.layers {
            let n = (layer.width() * layer.height()) as usize;
            let mut per_channel = Vec::new();
            // Order must match the channel-info order below: alpha, R, G, B.
            for ch in [3usize, 0, 1, 2] {
                let mut blob = Out::default();
                blob.u16(0); // compression: raw
                let plane = layer.plane(ch);
                debug_assert_eq!(plane.len(), n);
                blob.raw(&plane);
                per_channel.push(blob.0);
            }
            channel_blobs.push(per_channel);
        }

        for (layer, blobs) in self.layers.iter().zip(&channel_blobs) {
            layer_info.i32(layer.rect.1); // top
            layer_info.i32(layer.rect.0); // left
            layer_info.i32(layer.rect.3); // bottom
            layer_info.i32(layer.rect.2); // right

            layer_info.u16(4); // channel count
            for (id, blob) in [-1i16, 0, 1, 2].iter().zip(blobs) {
                layer_info.i16(*id); // -1 = alpha
                layer_info.u32(blob.len() as u32); // includes the compression field
            }

            layer_info.raw(b"8BIM");
            layer_info.raw(&layer.blend);
            layer_info.u8(layer.opacity);
            layer_info.u8(0); // clipping: base
            // Bit 1 set means *hidden*. Inverted from the obvious reading, and
            // an easy way to ship a file whose layers are all invisible.
            layer_info.u8(if layer.visible { 0x00 } else { 0x02 });
            layer_info.u8(0); // filler

            let mut extra = Out::default();
            extra.u32(0); // layer mask data: none
            extra.u32(0); // blending ranges: none
            // Legacy name: MacRoman, and effectively ASCII-only in practice.
            extra.pascal(&layer.name, 4);
            // Modern name: the 'luni' block, UTF-16BE. Without it a non-ASCII
            // layer name comes back mangled — Photoshop and psd-tools both
            // prefer this block when present, and fall back to the Pascal
            // string when it is absent.
            let utf16: Vec<u16> = layer.name.encode_utf16().collect();
            let mut luni = Out::default();
            luni.u32(utf16.len() as u32);
            for unit in &utf16 {
                luni.u16(*unit);
            }
            while luni.0.len() % 4 != 0 {
                luni.u8(0);
            }
            extra.raw(b"8BIM");
            extra.raw(b"luni");
            extra.u32(luni.0.len() as u32);
            extra.raw(&luni.0);
            layer_info.u32(extra.0.len() as u32);
            layer_info.raw(&extra.0);
        }

        for blobs in &channel_blobs {
            for blob in blobs {
                layer_info.raw(blob);
            }
        }

        // Layer info is padded to an even length.
        if layer_info.0.len() % 2 != 0 {
            layer_info.u8(0);
        }

        let mut layer_and_mask = Out::default();
        layer_and_mask.u32(layer_info.0.len() as u32);
        layer_and_mask.raw(&layer_info.0);
        layer_and_mask.u32(0); // global layer mask info: none

        o.u32(layer_and_mask.0.len() as u32);
        o.raw(&layer_and_mask.0);

        // --- Composite image data --------------------------------------------
        o.u16(0); // compression: raw
        o.raw(&self.composite_planar());

        o.0
    }
}
