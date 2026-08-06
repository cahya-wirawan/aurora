//! On-disk tile file format: a small versioned header plus an
//! `lz4_flex`-compressed (or, defensively, raw) payload.
//!
//! `spike/vertical-slice` proved the *paging* architecture but explicitly
//! skipped compression ("required by the real design... omitting it makes
//! the measurement worse than production, the safe direction") — this
//! module is the real thing.
//!
//! ```text
//! magic:      [u8; 4] = b"ATIL"
//! version:    u8      = 1
//! compressed: u8       0 = raw fallback, 1 = lz4_flex-compressed
//! reserved:   [u8; 2]
//! payload:    the rest of the file
//! ```
//!
//! `compressed` exists because `lz4` can *expand* incompressible data
//! (rare, but real for dense noise) — falling back to a raw payload
//! rather than storing an expanded blob is the defensive choice.
//!
//! Public (not `pub(crate)`) since [ADR 0009](../../../docs/adr/0009-aur-document-format.md)
//! names this exact format as what a `.aur` file's own per-tile ZIP
//! entries hold verbatim — `aurora_io::aur` calls [`encode`]/[`decode`]
//! directly rather than this crate inventing a second on-disk tile
//! encoding at the document level.

use crate::error::TileError;
use half::f16;

const MAGIC: [u8; 4] = *b"ATIL";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 8;

/// Serializes tile samples to the on-disk format described above.
#[must_use]
pub fn encode(texels: &[f16]) -> Vec<u8> {
    let raw = raw_bytes(texels);
    let compressed = lz4_flex::compress_prepend_size(&raw);
    let (flag, payload): (u8, Vec<u8>) = if compressed.len() < raw.len() {
        (1, compressed)
    } else {
        (0, raw)
    };

    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(flag);
    out.extend_from_slice(&[0, 0]); // reserved
    out.extend_from_slice(&payload);
    out
}

/// Parses the on-disk format back into tile samples.
///
/// # Errors
///
/// Returns [`TileError::CorruptFile`] if the header is missing/invalid,
/// the version is unsupported, or the payload fails to decompress or
/// parse as whole `f16` samples.
pub fn decode(bytes: &[u8]) -> Result<Vec<f16>, TileError> {
    let Some(magic) = bytes.get(0..4) else {
        return Err(TileError::CorruptFile(
            "file shorter than the header".to_owned(),
        ));
    };
    if magic != MAGIC.as_slice() {
        return Err(TileError::CorruptFile("bad magic".to_owned()));
    }
    let Some(&version) = bytes.get(4) else {
        return Err(TileError::CorruptFile(
            "file shorter than the header".to_owned(),
        ));
    };
    if version != VERSION {
        return Err(TileError::CorruptFile(format!(
            "unsupported tile file version {version}"
        )));
    }
    let Some(&flag) = bytes.get(5) else {
        return Err(TileError::CorruptFile(
            "file shorter than the header".to_owned(),
        ));
    };
    let Some(payload) = bytes.get(HEADER_LEN..) else {
        return Err(TileError::CorruptFile(
            "file shorter than the header".to_owned(),
        ));
    };

    let raw = match flag {
        0 => payload.to_vec(),
        1 => lz4_flex::decompress_size_prepended(payload)
            .map_err(|err| TileError::CorruptFile(format!("lz4 decompress failed: {err}")))?,
        other => {
            return Err(TileError::CorruptFile(format!(
                "unknown compression flag {other}"
            )));
        }
    };
    from_raw_bytes(&raw)
}

/// `f16` samples, little-endian, no bulk reinterpret-casting — avoids any
/// alignment assumption about a freshly-decompressed `Vec<u8>`.
fn raw_bytes(texels: &[f16]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(texels.len() * 2);
    for texel in texels {
        raw.extend_from_slice(&texel.to_le_bytes());
    }
    raw
}

fn from_raw_bytes(raw: &[u8]) -> Result<Vec<f16>, TileError> {
    if !raw.len().is_multiple_of(2) {
        return Err(TileError::CorruptFile(format!(
            "payload length {} is not a whole number of f16 samples",
            raw.len()
        )));
    }
    let mut texels = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks_exact(2) {
        let bytes: [u8; 2] = match pair.try_into() {
            Ok(bytes) => bytes,
            Err(_) => unreachable!("chunks_exact(2) guarantees a length-2 slice"),
        };
        texels.push(f16::from_le_bytes(bytes));
    }
    Ok(texels)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use half::f16;

    #[test]
    fn round_trips_bit_exact() {
        // Matches spike/FINDINGS.md's own proven property ("f16 in, f16
        // out, no conversion anywhere") -- now with real compression in
        // the loop, not just a raw dump.
        let texels: Vec<f16> = (0..1000).map(|i| f16::from_f32(i as f32 * 0.001)).collect();
        let encoded = encode(&texels);
        let decoded = match decode(&encoded) {
            Ok(decoded) => decoded,
            Err(err) => unreachable!("decoding output this test just encoded must succeed: {err}"),
        };
        assert_eq!(texels.len(), decoded.len());
        for (a, b) in texels.iter().zip(decoded.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    #[test]
    fn uniform_data_compresses() {
        // Blank/uniform tiles are the common case (most of a huge sparse
        // canvas is untouched) -- these must actually hit the compressed
        // path, not just fall back to raw.
        let texels = vec![f16::from_f32(0.0); 65_536 * 4];
        let encoded = encode(&texels);
        assert!(
            encoded.len() < texels.len() * 2,
            "uniform data must compress smaller than raw"
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode(&[f16::from_f32(1.0)]);
        let Some(first) = bytes.get_mut(0) else {
            unreachable!("just-encoded buffer always has a first byte");
        };
        *first = b'X';
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(decode(&[0, 1, 2]).is_err());
    }
}
