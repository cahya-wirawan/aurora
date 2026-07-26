//! A minimal reader/writer for Photoshop's "Descriptor" binary format —
//! the generic serialization Photoshop reuses across many features (text
//! layers, effects, gradients, adjustment parameters). `TySh` (type-tool
//! settings) embeds two of these.
//!
//! Scope: only the five value types confirmed present in a real Photoshop
//! file's `TySh`/`warp` descriptors (String, Double, Integer, Enumerated,
//! RawData), plus nested descriptors for `warp`. Anything else is out of
//! scope and this reader will error rather than silently mis-parse — for a
//! format this obscure, failing loudly beats guessing.
//!
//! Every byte offset here was confirmed by reading `psd-tools`' own
//! `psd/descriptor.py` before writing a line of this file, then verified
//! against `reference/tysh.bin` — a real `TySh` block extracted from a
//! Photoshop-authored PSD (`reference/text.psd`, from psd-tools' own test
//! fixtures) — not inferred from spec text alone. See `spike/psd-write/
//! FINDINGS.md` for why that distinction matters here specifically.

use std::io::{self, Cursor, Read, Write};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// OSType `TEXT`: u32 char count + UTF-16BE.
    Str(String),
    /// OSType `doub`: 8-byte big-endian f64.
    Double(f64),
    /// OSType `long`: 4-byte big-endian i32.
    Int(i32),
    /// OSType `enum`: two length-prefixed keys (type category, then value).
    Enum { type_id: Vec<u8>, value: Vec<u8> },
    /// OSType `tdta`: u32 length + raw bytes. Wraps `EngineData` untouched —
    /// its own text-based format is a separate, unimplemented format here.
    Raw(Vec<u8>),
    /// OSType `Objc`: a nested descriptor (`warp` is one of these).
    Nested(Descriptor),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub name: String,
    pub class_id: Vec<u8>,
    /// Insertion order preserved — Photoshop's own descriptors are ordered
    /// and round-trip position matters for byte-identical output.
    pub items: Vec<(Vec<u8>, Value)>,
}

/// A `Descriptor` with an extra leading `u32` version field (always 16).
///
/// This is what `TySh`'s `text_data` and `warp` fields actually are —
/// confirmed by reading `psd-tools`' `DescriptorBlock` class, which is
/// *not* the same as the plain `Descriptor` used for nested `Objc` values
/// inside an item list. Missing this field is what caused this parser's
/// first attempt to desync a few bytes in and read garbage as if it were
/// real structure — the exact "plausible but wrong" failure mode this
/// spike's earlier findings warned about, just one layer deeper.
#[derive(Debug, Clone, PartialEq)]
pub struct DescriptorBlock {
    pub version: u32,
    pub descriptor: Descriptor,
}

impl DescriptorBlock {
    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let version = read_u32(r)?;
        let descriptor = Descriptor::read(r)?;
        Ok(Self {
            version,
            descriptor,
        })
    }

    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.version.to_be_bytes())?;
        self.descriptor.write(w)
    }

    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.descriptor.get(key)
    }

    pub fn set(&mut self, key: &[u8], value: Value) -> bool {
        self.descriptor.set(key, value)
    }
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}
fn read_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}
fn read_f64<R: Read>(r: &mut R) -> io::Result<f64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(f64::from_be_bytes(b))
}

/// `read_length_and_key` in psd-tools: u32 length; if the key is exactly 4
/// bytes, `length` may legitimately be either 0 or 4 (both read 4 bytes) —
/// see FINDINGS.md. We always *write* the explicit true length, which any
/// reader following the same `length.max(4-if-zero)` rule accepts identically.
fn read_length_and_key<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let len = read_u32(r)?;
    let n = if len == 0 { 4 } else { len as usize };
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}
fn write_length_and_key<W: Write>(w: &mut W, key: &[u8]) -> io::Result<()> {
    w.write_all(&(key.len() as u32).to_be_bytes())?;
    w.write_all(key)
}

fn read_unicode_string<R: Read>(r: &mut R) -> io::Result<String> {
    let n = read_u32(r)? as usize;
    let mut buf = vec![0u8; n * 2];
    r.read_exact(&mut buf)?;
    let units: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
fn write_unicode_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    let units: Vec<u16> = s.encode_utf16().collect();
    w.write_all(&(units.len() as u32).to_be_bytes())?;
    for u in units {
        w.write_all(&u.to_be_bytes())?;
    }
    Ok(())
}

impl Value {
    fn ostype(&self) -> &'static [u8; 4] {
        match self {
            Value::Str(_) => b"TEXT",
            Value::Double(_) => b"doub",
            Value::Int(_) => b"long",
            Value::Enum { .. } => b"enum",
            Value::Raw(_) => b"tdta",
            Value::Nested(_) => b"Objc",
        }
    }

    fn read<R: Read>(ostype: &[u8; 4], r: &mut R) -> io::Result<Self> {
        match ostype {
            b"TEXT" => Ok(Value::Str(read_unicode_string(r)?)),
            b"doub" => Ok(Value::Double(read_f64(r)?)),
            b"long" => Ok(Value::Int(read_i32(r)?)),
            b"enum" => {
                let type_id = read_length_and_key(r)?;
                let value = read_length_and_key(r)?;
                Ok(Value::Enum { type_id, value })
            }
            b"tdta" => {
                let n = read_u32(r)? as usize;
                let mut buf = vec![0u8; n];
                r.read_exact(&mut buf)?;
                Ok(Value::Raw(buf))
            }
            b"Objc" => Ok(Value::Nested(Descriptor::read(r)?)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported Descriptor OSType {:?} — out of scope for this spike, \
                     see FINDINGS.md; failing loudly rather than mis-parsing",
                    String::from_utf8_lossy(other)
                ),
            )),
        }
    }

    fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self {
            Value::Str(s) => write_unicode_string(w, s),
            Value::Double(d) => w.write_all(&d.to_be_bytes()),
            Value::Int(i) => w.write_all(&i.to_be_bytes()),
            Value::Enum { type_id, value } => {
                write_length_and_key(w, type_id)?;
                write_length_and_key(w, value)
            }
            Value::Raw(bytes) => {
                w.write_all(&(bytes.len() as u32).to_be_bytes())?;
                w.write_all(bytes)
            }
            Value::Nested(d) => d.write(w),
        }
    }
}

impl Descriptor {
    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let name = read_unicode_string(r)?;
        let class_id = read_length_and_key(r)?;
        let count = read_u32(r)?;
        let mut items = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let key = read_length_and_key(r)?;
            let mut tag = [0u8; 4];
            r.read_exact(&mut tag)?;
            let value = Value::read(&tag, r)?;
            items.push((key, value));
        }
        Ok(Descriptor {
            name,
            class_id,
            items,
        })
    }

    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        write_unicode_string(w, &self.name)?;
        write_length_and_key(w, &self.class_id)?;
        w.write_all(&(self.items.len() as u32).to_be_bytes())?;
        for (key, value) in &self.items {
            write_length_and_key(w, key)?;
            w.write_all(value.ostype())?;
            value.write(w)?;
        }
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn set(&mut self, key: &[u8], value: Value) -> bool {
        for (k, v) in &mut self.items {
            if k.as_slice() == key {
                *v = value;
                return true;
            }
        }
        false
    }
}

/// The full `TySh` payload: version, transform, the text descriptor, the
/// warp descriptor, and a bounding box. Byte layout confirmed from
/// `psd-tools`' `TypeToolObjectSetting.read`/`write` (`psd/tagged_blocks.py`).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeToolObjectSetting {
    pub version: u16,
    pub transform: [f64; 6],
    pub text_version: u16,
    pub text_data: DescriptorBlock,
    pub warp_version: u16,
    pub warp: DescriptorBlock,
    pub bbox: (i32, i32, i32, i32), // left, top, right, bottom
}

impl TypeToolObjectSetting {
    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut u16buf = [0u8; 2];
        r.read_exact(&mut u16buf)?;
        let version = u16::from_be_bytes(u16buf);

        let mut transform = [0.0; 6];
        for t in &mut transform {
            *t = read_f64(r)?;
        }

        r.read_exact(&mut u16buf)?;
        let text_version = u16::from_be_bytes(u16buf);
        let text_data = DescriptorBlock::read(r)?;

        r.read_exact(&mut u16buf)?;
        let warp_version = u16::from_be_bytes(u16buf);
        let warp = DescriptorBlock::read(r)?;

        let left = read_i32(r)?;
        let top = read_i32(r)?;
        let right = read_i32(r)?;
        let bottom = read_i32(r)?;

        Ok(Self {
            version,
            transform,
            text_version,
            text_data,
            warp_version,
            warp,
            bbox: (left, top, right, bottom),
        })
    }

    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.version.to_be_bytes())?;
        for t in &self.transform {
            w.write_all(&t.to_be_bytes())?;
        }
        w.write_all(&self.text_version.to_be_bytes())?;
        self.text_data.write(w)?;
        w.write_all(&self.warp_version.to_be_bytes())?;
        self.warp.write(w)?;
        w.write_all(&self.bbox.0.to_be_bytes())?;
        w.write_all(&self.bbox.1.to_be_bytes())?;
        w.write_all(&self.bbox.2.to_be_bytes())?;
        w.write_all(&self.bbox.3.to_be_bytes())?;
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.write(&mut buf).expect("write to Vec is infallible");
        buf
    }

    /// `Txt `'s value as psd-tools reads it — a real reader's own text
    /// extraction goes through this exact field, confirmed by reading its
    /// `TypeLayer.text` property (`api/layers.py`) against the real file.
    pub fn text(&self) -> Option<&str> {
        match self.text_data.get(b"Txt ") {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn set_text(&mut self, s: impl Into<String>) -> bool {
        self.text_data.set(b"Txt ", Value::Str(s.into()))
    }
}

pub fn parse_fixture(bytes: &[u8]) -> io::Result<TypeToolObjectSetting> {
    TypeToolObjectSetting::read(&mut Cursor::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `TySh` block, extracted (via psd-tools) from `text.psd` — one
    /// of psd-tools' own test fixtures, genuinely authored by Photoshop, not
    /// synthesized. See `reference/README.md`.
    const REAL_TYSH: &[u8] = include_bytes!("../reference/tysh.bin");

    #[test]
    fn parses_real_photoshop_bytes() {
        let tysh = parse_fixture(REAL_TYSH).expect("a real Photoshop TySh block must parse");
        assert_eq!(tysh.version, 1);
        assert_eq!(tysh.text_version, 50);
        assert_eq!(tysh.text(), Some("Line 1\rLine 2\rLine 3 and text\u{0}"));
        // The five value types actually present, confirmed in FINDINGS.md by
        // reading the real file with psd-tools before writing this parser.
        assert!(matches!(
            tysh.text_data.get(b"textGridding"),
            Some(Value::Enum { .. })
        ));
        assert!(matches!(
            tysh.text_data.get(b"TextIndex"),
            Some(Value::Int(0))
        ));
        assert!(matches!(
            tysh.text_data.get(b"EngineData"),
            Some(Value::Raw(_))
        ));
        assert!(matches!(
            tysh.warp.get(b"warpValue"),
            Some(Value::Double(_))
        ));
    }

    #[test]
    fn round_trips_same_length_but_not_byte_identical() {
        // Unlike the plain layer-record format (ADR 0004, groups spike), the
        // Descriptor format has a genuine ambiguity built into its own spec:
        // `read_length_and_key` treats a length field of exactly 0 as
        // shorthand for "the key is the following 4 bytes" — so a reader
        // must accept EITHER 0 or the true length 4 for a well-known 4-byte
        // term, and both decode to the identical key. Real Photoshop writes
        // the 0-shorthand for terms it recognizes (confirmed directly: the
        // real file's own classID field is `0x00000000` + "TxLr", not
        // `0x00000004` + "TxLr"); this writer always writes the explicit
        // length instead of replicating Photoshop's own lookup table of
        // known terms, which is unnecessary complexity for a spike.
        //
        // So the correct invariant to test is NOT byte-identical output —
        // that bar doesn't hold for this format even for a fully-correct
        // implementation. It's: identical *length* (the encoding choice
        // never changes byte count, only 4 specific bytes' value), and a
        // semantically identical structure when re-parsed by this same
        // reader.
        let tysh = parse_fixture(REAL_TYSH).expect("parse");
        let out = tysh.to_bytes();
        assert_eq!(
            out.len(),
            REAL_TYSH.len(),
            "re-serialized length differs from the real file's own bytes -- \
             this WOULD indicate a real bug, unlike raw byte inequality"
        );
        let reparsed = parse_fixture(&out).expect("re-parse of our own output");
        assert_eq!(
            reparsed, tysh,
            "re-parsed structure differs semantically -- a real bug, not just \
             the documented length-encoding choice"
        );
    }

    #[test]
    fn patches_text_and_stays_internally_consistent() {
        let mut tysh = parse_fixture(REAL_TYSH).expect("parse");
        assert!(tysh.set_text("Aurora spike\u{0}"));

        // Re-parse what we just wrote with our own reader — this is only
        // self-consistency, not independent verification. The independent
        // verification (psd-tools, sips, on a full assembled file) is the
        // Python proof-of-concept documented in FINDINGS.md; ports to a full
        // Rust file-level splice are follow-on work, not done here.
        let bytes = tysh.to_bytes();
        let reparsed = parse_fixture(&bytes).expect("re-parse of our own patched output");
        assert_eq!(reparsed.text(), Some("Aurora spike\u{0}"));

        // Confirm patching a field we didn't touch stayed untouched.
        assert_eq!(reparsed.warp.get(b"warpValue"), tysh.warp.get(b"warpValue"));
    }

    #[test]
    fn rejects_unknown_types_loudly_instead_of_guessing() {
        // b"XXXX" is not one of the five OSTypes this parser supports.
        // Silently skipping or mis-parsing an unrecognized type is exactly
        // the failure mode this spike's earlier findings warn about; erroring
        // is the correct behavior for a format this poorly specified.
        let mut bogus = Vec::new();
        bogus.extend_from_slice(b"XXXX");
        let err = Value::read(
            bogus[..4].try_into().unwrap(),
            &mut Cursor::new(&bogus[4..]),
        );
        assert!(err.is_err());
    }
}
