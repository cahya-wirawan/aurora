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

/// The most bytes [`encode`] can produce for one tile: the header plus
/// the raw payload, which it stores whenever compression does not help.
pub const MAX_ENCODED_LEN: usize = HEADER_LEN + crate::SAMPLES * std::mem::size_of::<f16>();

/// The one and only decoded payload length [`decode`] accepts: exactly
/// one `TILE * TILE` f16 RGBA tile ([`crate::SAMPLES`] samples, two
/// bytes each). Nothing this module ever encodes is any other size, so
/// anything else on disk is corruption.
///
/// It is checked in two places, for two different reasons, and both are
/// load-bearing.
///
/// **Before decompression, against lz4's own size prefix.** This is a
/// real check against untrusted input, not a tidiness one.
/// `lz4_flex::decompress_size_prepended` takes the decompressed length
/// from a four-byte little-endian prefix *inside the compressed frame*
/// and allocates that much up front (0.11.6, `block::decompress`:
/// `vec![0; min_uncompressed_size]`) before it has decoded a single
/// byte. That prefix is attacker-controlled and completely independent
/// of how large the compressed blob is, so it evades any cap applied to
/// the *outer* container entry: measured here, a 26-byte input claiming
/// `3_000_000_000` moved this process's `VmSize` from 3.3 MB to 2.93 GB
/// in 7 µs, and still returned `Ok`. A `.aur` file's own tile entries
/// are one such untrusted input, read on `aurora-app`'s pre-window
/// startup path — and an allocation that big failing is an abort, not an
/// error. Checking the prefix before handing it to `lz4_flex` is what
/// keeps this bounded. Until 0.52.1 that check was an *upper* bound
/// (`declared > MAX_DECOMPRESSED_BYTES`); now that the exact legitimate
/// size is known it is an equality, which rejects an under-sized claim
/// before decompression is even attempted rather than after.
///
/// **After decoding, against the real payload length.** This is what
/// stops a short-but-well-formed tile file reaching a caller as a short
/// `&[f16]`: `aurora-app`'s own `write_composited`
/// (`recomposite_visible_tiles`) ends in
/// `dest.texels_mut().copy_from_slice(composited)`, which panics on a
/// length mismatch, and `aurora_render::composite_layer_into` zips its
/// two slices, so a short buffer there composites only part of a tile
/// with no error at all. Every `Tile` this crate hands out is
/// `SAMPLES`-long (`Tile::blank`, or `Tile::from_texels` fed only by
/// [`decode`]), so enforcing it here is what makes that true for the
/// paged-in case too. On the raw (`compressed: 0`) branch it runs
/// *before* the payload is copied, so an arbitrarily large corrupt file
/// is rejected rather than duplicated into memory first.
const EXPECTED_DECODED_BYTES: usize = crate::SAMPLES * 2;

/// Serializes one whole tile's samples to the on-disk format described
/// above.
///
/// [`decode`] accepts exactly `SAMPLES * 2` bytes and nothing
/// else, so encoding any other length produces a file this module can
/// never read back. Every production caller
/// (`TileStore::make_room`, `aurora_io::aur::write`) passes a real
/// `Tile`'s own texel buffer, which is `crate::SAMPLES`-long by
/// construction; the `debug_assert_eq!` is what catches a future one
/// that does not, in debug builds, instead of silently writing an
/// undecodable tile. Corruption fixtures that deliberately need a
/// wrong-sized payload go through this module's own `encode_any_length`.
#[must_use]
pub fn encode(texels: &[f16]) -> Vec<u8> {
    debug_assert_eq!(
        texels.len(),
        crate::SAMPLES,
        "`encode`/`decode` are asymmetric: `decode` accepts exactly one whole tile, so encoding \
         any other length writes a file that can never be read back"
    );
    encode_any_length(texels)
}

/// [`encode`] without the whole-tile check — the shared body, kept
/// separate so this module's own corruption fixtures can build a
/// well-formed-but-wrong-sized ATIL file the way a crash mid-write or a
/// truncated scratch file really does. Production code wants [`encode`].
pub(crate) fn encode_any_length(texels: &[f16]) -> Vec<u8> {
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

/// Buffers [`encode_with`] reuses between tiles (0.189.0): the raw bytes
/// and the compressed block. Allocating and first-touching those
/// half-megabyte buffers for every tile was most of the time of encoding
/// a tile, and the page faults serialize across threads; a caller that
/// encodes many tiles keeps one scratch per thread.
#[derive(Debug, Default)]
pub struct EncodeScratch {
    raw: Vec<u8>,
    block: Vec<u8>,
}

/// [`encode`], byte for byte, through reusable buffers: the same header,
/// the same `lz4_flex` block with its size prepended, the same raw
/// fallback when compression does not shrink the payload. Only the result
/// itself is allocated, at its exact size.
#[must_use]
pub fn encode_with(texels: &[f16], scratch: &mut EncodeScratch) -> Vec<u8> {
    debug_assert_eq!(texels.len(), crate::SAMPLES);
    scratch.raw.clear();
    scratch.raw.reserve(texels.len() * 2);
    extend_le_bytes(texels, &mut scratch.raw);
    let max = lz4_flex::block::get_maximum_output_size(scratch.raw.len());
    if scratch.block.len() < max {
        scratch.block.resize(max, 0);
    }
    let compressed = lz4_flex::block::compress_into(&scratch.raw, &mut scratch.block).ok();
    let prefix = u32::try_from(scratch.raw.len()).ok();
    let (flag, payload_len) = match (compressed, prefix) {
        (Some(n), Some(_)) if n + 4 < scratch.raw.len() => (1_u8, n + 4),
        _ => (0_u8, scratch.raw.len()),
    };
    let mut out = Vec::with_capacity(HEADER_LEN + payload_len);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(flag);
    out.extend_from_slice(&[0, 0]); // reserved
    match (flag, compressed, prefix) {
        (1, Some(n), Some(prefix)) => {
            out.extend_from_slice(&prefix.to_le_bytes());
            out.extend_from_slice(scratch.block.get(..n).unwrap_or(&[]));
        }
        _ => out.extend_from_slice(&scratch.raw),
    }
    out
}

/// Parses the on-disk format back into tile samples.
///
/// # Errors
///
/// Returns [`TileError::CorruptFile`] if the header is missing/invalid,
/// the version is unsupported, or the payload fails to decompress, or
/// decodes to anything other than exactly one whole tile
/// ([`crate::SAMPLES`] samples).
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
        0 => {
            // Checked *before* the copy, not after: a corrupt raw
            // payload can be any size at all, and duplicating it into an
            // owned `Vec` first would be an unbounded allocation driven
            // by the file's own length -- the same "check the size, then
            // allocate" doctrine `EXPECTED_DECODED_BYTES` already
            // documents for the compressed branch below.
            length_checked(payload)?;
            payload.to_vec()
        }
        1 => {
            // See `EXPECTED_DECODED_BYTES`: the size prefix decides how
            // much `lz4_flex` allocates, so it is checked here rather
            // than trusted.
            let Some(prefix) = payload.get(0..4) else {
                return Err(TileError::CorruptFile(
                    "compressed payload is shorter than its own size prefix".to_owned(),
                ));
            };
            let declared = match <[u8; 4]>::try_from(prefix) {
                Ok(bytes) => u32::from_le_bytes(bytes),
                Err(_) => unreachable!("a 4-byte slice always converts to [u8; 4]"),
            };
            if u64::from(declared) != EXPECTED_DECODED_BYTES as u64 {
                return Err(TileError::CorruptFile(format!(
                    "compressed payload claims {declared} decompressed bytes, not the \
                     {EXPECTED_DECODED_BYTES} bytes of exactly one {}-sample tile",
                    crate::SAMPLES
                )));
            }
            lz4_flex::decompress_size_prepended(payload)
                .map_err(|err| TileError::CorruptFile(format!("lz4 decompress failed: {err}")))?
        }
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
    extend_le_bytes(texels, &mut raw);
    raw
}

/// Appends `texels` to `raw` as little-endian bytes, two per sample.
///
/// On a little-endian target that is the samples' own memory, appended in
/// one copy (60 FPS round 4): the per-sample `extend_from_slice` loop it
/// replaced cost about 0.25 ms a tile in the dev test profile, on every
/// eviction. Pinned against that loop over every `f16` bit pattern.
#[cfg(target_endian = "little")]
fn extend_le_bytes(texels: &[f16], raw: &mut Vec<u8>) {
    raw.extend_from_slice(bytemuck::cast_slice(texels));
}

/// Big-endian spelling of [`extend_le_bytes`]: one sample at a time.
#[cfg(not(target_endian = "little"))]
fn extend_le_bytes(texels: &[f16], raw: &mut Vec<u8>) {
    for texel in texels {
        raw.extend_from_slice(&texel.to_le_bytes());
    }
}

/// The exact-length half of [`EXPECTED_DECODED_BYTES`]'s own contract,
/// factored out so the raw branch can run it *before* copying the
/// payload and [`from_raw_bytes`] can still run it on whatever
/// `lz4_flex` actually produced.
fn length_checked(raw: &[u8]) -> Result<(), TileError> {
    if raw.len() != EXPECTED_DECODED_BYTES {
        return Err(TileError::CorruptFile(format!(
            "payload decoded to {} bytes, not the {EXPECTED_DECODED_BYTES} bytes of exactly one \
             {}-sample tile",
            raw.len(),
            crate::SAMPLES
        )));
    }
    Ok(())
}

fn from_raw_bytes(raw: &[u8]) -> Result<Vec<f16>, TileError> {
    // Defence in depth on the compressed branch, where the size prefix
    // has already been checked against `EXPECTED_DECODED_BYTES` but the
    // bytes `lz4_flex` really produced have not; a no-op repeat on the
    // raw branch, which checked the same length before copying.
    length_checked(raw)?;
    Ok(texels_from_le_bytes(raw))
}

/// The samples of little-endian `raw`, two bytes each (an odd trailing
/// byte, which [`length_checked`] rules out, is ignored). On a
/// little-endian target one copy (60 FPS round 4; the per-pair loop it
/// replaced is the reference it is pinned against, over every pattern).
#[cfg(target_endian = "little")]
fn texels_from_le_bytes(raw: &[u8]) -> Vec<f16> {
    use half::vec::HalfBitsVecExt as _;
    let even = raw.get(..raw.len() / 2 * 2).unwrap_or_default();
    let mut texels: Vec<f16> = vec![0u16; even.len() / 2].reinterpret_into();
    // Equal lengths by construction: `even` is twice `texels.len()` bytes.
    bytemuck::cast_slice_mut::<f16, u8>(&mut texels).copy_from_slice(even);
    texels
}

/// Big-endian spelling of [`texels_from_le_bytes`]: one pair at a time.
#[cfg(not(target_endian = "little"))]
fn texels_from_le_bytes(raw: &[u8]) -> Vec<f16> {
    let mut texels = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks_exact(2) {
        if let [lo, hi] = pair {
            texels.push(f16::from_le_bytes([*lo, *hi]));
        }
    }
    texels
}

#[cfg(test)]
mod tests {
    /// The 0.268.0 encoder, verbatim but for its name: the per-sample raw
    /// byte loop, then the same lz4 and header. The reference every tile
    /// file written before 60 FPS round 4 came from.
    fn encode_0268(texels: &[half::f16]) -> Vec<u8> {
        let mut raw = Vec::with_capacity(texels.len() * 2);
        for texel in texels {
            raw.extend_from_slice(&texel.to_le_bytes());
        }
        let compressed = lz4_flex::compress_prepend_size(&raw);
        let (flag, payload): (u8, Vec<u8>) = if compressed.len() < raw.len() {
            (1, compressed)
        } else {
            (0, raw)
        };
        let mut out = Vec::with_capacity(super::HEADER_LEN + payload.len());
        out.extend_from_slice(&super::MAGIC);
        out.push(super::VERSION);
        out.push(flag);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&payload);
        out
    }

    /// The 0.268.0 decoder's sample loop over a raw payload.
    fn samples_0268(raw: &[u8]) -> Vec<u16> {
        raw.chunks_exact(2)
            .map(|p| match p {
                [lo, hi] => half::f16::from_le_bytes([*lo, *hi]).to_bits(),
                _ => unreachable!("chunks_exact(2)"),
            })
            .collect()
    }

    /// Tiles covering every `f16` bit pattern (each four times, in four
    /// arrangements: NaN payloads, -0, subnormals, infinities included),
    /// blank and constant tiles, and seeded random tiles both compressible
    /// and not.
    fn round4_tiles() -> Vec<(String, Vec<half::f16>)> {
        let n = crate::SAMPLES;
        let from = |f: &dyn Fn(usize) -> u16| -> Vec<half::f16> {
            (0..n).map(|i| half::f16::from_bits(f(i))).collect()
        };
        let mut state = 0x9e37_79b9u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let random: Vec<u16> = (0..n).map(|_| next() as u16).collect();
        let lumpy: Vec<u16> = (0..n)
            .map(|i| if i % 64 < 8 { next() as u16 } else { 0x3c00 })
            .collect();
        vec![
            ("every pattern".into(), from(&|i| i as u16)),
            (
                "every pattern x7".into(),
                from(&|i| (i as u16).wrapping_mul(7)),
            ),
            (
                "every pattern reversed".into(),
                from(&|i| u16::MAX - i as u16),
            ),
            (
                "every pattern high byte first".into(),
                from(&|i| (i as u16).rotate_left(8)),
            ),
            ("blank".into(), crate::Tile::blank().texels().to_vec()),
            ("negative zero".into(), from(&|_| 0x8000)),
            ("signalling NaN".into(), from(&|_| 0x7c01)),
            (
                "random".into(),
                from(&|i| random.get(i).copied().unwrap_or(0)),
            ),
            (
                "lumpy".into(),
                from(&|i| lumpy.get(i).copied().unwrap_or(0)),
            ),
        ]
    }

    /// 60 FPS round 4: what reaches the scratch disk, spill files, the
    /// autosave and `.aur` files is byte for byte what 0.268.0 wrote, old
    /// files decode to the same bits, and every tile round-trips.
    #[test]
    fn encoded_bytes_are_identical_to_the_0268_encoder_and_old_files_decode_the_same() {
        let mut scratch = super::EncodeScratch::default();
        for (what, texels) in round4_tiles() {
            let old = encode_0268(&texels);
            assert_eq!(super::encode(&texels), old, "{what}: encode");
            assert_eq!(
                super::encode_with(&texels, &mut scratch),
                old,
                "{what}: encode_with"
            );
            let bits: Vec<u16> = texels.iter().map(|t| t.to_bits()).collect();
            let decoded = match super::decode(&old) {
                Ok(d) => d,
                Err(err) => unreachable!("{what}: an old file decodes: {err}"),
            };
            let decoded: Vec<u16> = decoded.iter().map(|t| t.to_bits()).collect();
            assert_eq!(decoded, bits, "{what}: old bytes decode to the same bits");
            // The old decoder's loop over the raw payload agrees too.
            let raw = match old.get(5) {
                Some(1) => old
                    .get(super::HEADER_LEN..)
                    .and_then(|p| lz4_flex::decompress_size_prepended(p).ok())
                    .unwrap_or_default(),
                _ => old.get(super::HEADER_LEN..).unwrap_or_default().to_vec(),
            };
            assert_eq!(
                samples_0268(&raw),
                bits,
                "{what}: the 0.268.0 loop reads the payload"
            );
        }
    }

    /// 60 FPS round 4: the bulk byte conversions against the per-sample
    /// loops they replaced, over every `f16` bit pattern, at aligned and
    /// unaligned starts.
    #[test]
    fn the_bulk_byte_conversions_match_the_per_sample_loops_for_every_pattern() {
        use half::f16;
        let texels: Vec<f16> = (0..=u16::MAX).map(f16::from_bits).collect();
        let mut reference = Vec::new();
        for texel in &texels {
            reference.extend_from_slice(&texel.to_le_bytes());
        }
        let mut raw = vec![0xa5u8, 0x5a];
        super::extend_le_bytes(&texels, &mut raw);
        assert_eq!(raw.get(..2), Some([0xa5u8, 0x5a].as_slice()), "appends");
        assert_eq!(raw.get(2..), Some(reference.as_slice()));
        assert_eq!(super::raw_bytes(&texels), reference);
        for start in [0usize, 1, 2, 3] {
            let bytes = raw.get(2 + start..).unwrap_or_default();
            let mut want = Vec::new();
            for pair in bytes.chunks_exact(2) {
                if let [lo, hi] = pair {
                    want.push(f16::from_le_bytes([*lo, *hi]).to_bits());
                }
            }
            let got: Vec<u16> = super::texels_from_le_bytes(bytes)
                .iter()
                .map(|t| t.to_bits())
                .collect();
            assert_eq!(got, want, "start {start}");
        }
        let round: Vec<u16> = super::texels_from_le_bytes(&reference)
            .iter()
            .map(|t| t.to_bits())
            .collect();
        assert_eq!(round, (0..=u16::MAX).collect::<Vec<_>>());
    }

    #[test]
    fn encode_with_is_byte_identical_to_encode_for_every_kind_of_tile() {
        let mut scratch = super::EncodeScratch::default();
        let mut seed = 5_u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            half::f16::from_bits((seed >> 8) as u16)
        };
        let zero = vec![half::f16::ZERO; crate::SAMPLES];
        let solid = vec![half::f16::from_f32(0.25); crate::SAMPLES];
        let noisy: Vec<half::f16> = (0..crate::SAMPLES).map(|_| noise()).collect();
        let mut half_noisy = zero.clone();
        for slot in half_noisy.iter_mut().take(crate::SAMPLES / 3) {
            *slot = noise();
        }
        // The scratch is reused across kinds and in both orders, so a
        // stale tail from a bigger block cannot leak into a smaller one.
        for texels in [&zero, &noisy, &solid, &half_noisy, &zero, &noisy] {
            assert_eq!(
                super::encode_with(texels, &mut scratch),
                super::encode(texels)
            );
        }
    }

    use super::{decode, encode, encode_any_length};
    use half::f16;

    /// One whole tile of high-entropy `f16` bit patterns, from a
    /// deterministic xorshift64\* stream -- no `rand` dependency, and
    /// identical on every run and every platform, so a failure here is
    /// reproducible.
    ///
    /// The point is that lz4 cannot usefully compress it, which is what
    /// forces [`encode`] down its `compressed: 0` raw fallback.
    /// Comparison is by `to_bits`, so the NaN patterns this inevitably
    /// produces are a feature: they are exactly the values a naive
    /// float-comparing round trip would get wrong.
    fn incompressible_tile() -> Vec<f16> {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut texels = Vec::with_capacity(crate::SAMPLES);
        for _ in 0..crate::SAMPLES {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let bits = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 48) as u16;
            texels.push(f16::from_bits(bits));
        }
        texels
    }

    #[test]
    fn round_trips_bit_exact() {
        // Matches spike/FINDINGS.md's own proven property ("f16 in, f16
        // out, no conversion anywhere") -- now with real compression in
        // the loop, not just a raw dump.
        // A real tile's worth of samples, cycling the same 1000 distinct
        // values the smaller fixture used -- `decode` now accepts exactly
        // one whole tile and nothing else. A repeating pattern that long
        // is trivially compressible, so this exercises the *compressed*
        // branch; `round_trips_incompressible_data_through_the_raw_branch`
        // below is the raw branch's own round trip.
        let texels: Vec<f16> = (0..crate::SAMPLES)
            .map(|i| f16::from_f32((i % 1000) as f32 * 0.001))
            .collect();
        let encoded = encode(&texels);
        assert!(
            encoded.get(5) == Some(&1),
            "this test covers the compressed branch; a repeating pattern must take it"
        );
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
    fn round_trips_incompressible_data_through_the_raw_branch() {
        // The `compressed: 0` fallback is not a defensive dead branch: a
        // dense photographic or noise tile really does expand under lz4,
        // and this module's whole reason for carrying a raw fallback is
        // that such tiles exist. Every other success-path test here
        // feeds compressible data, so without this one the raw branch's
        // *decode* logic has no test that fails when it breaks -- proven
        // by mutation: sabotaging it left the whole workspace green.
        let texels = incompressible_tile();
        let encoded = encode(&texels);
        assert!(
            encoded.get(5) == Some(&0),
            "this test needs the raw branch; high-entropy data must expand under lz4 and take it"
        );
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
        // A *full* tile, deliberately: encoded any shorter, the exact-
        // length check would reject this file on its own and the test
        // would keep passing with the magic check deleted outright.
        // Asserting the message names the magic is the other half of the
        // same point.
        let mut bytes = encode(&vec![f16::from_f32(1.0); crate::SAMPLES]);
        let Some(first) = bytes.get_mut(0) else {
            unreachable!("just-encoded buffer always has a first byte");
        };
        *first = b'X';
        match decode(&bytes) {
            Err(crate::TileError::CorruptFile(message)) => {
                assert!(
                    message.contains("bad magic"),
                    "an otherwise valid tile with a corrupted magic must be rejected *for* the \
                     magic, not for anything else: {message}"
                );
            }
            other => unreachable!("expected CorruptFile, got {other:?}"),
        }
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(decode(&[0, 1, 2]).is_err());
    }

    #[test]
    fn rejects_a_compressed_payload_claiming_more_than_one_tile_of_output() {
        // The lz4 size prefix is inside the compressed frame, so it is
        // free to claim any size at all regardless of how few bytes the
        // frame really occupies -- and `decompress_size_prepended`
        // allocates that claim before decoding anything. Verified
        // against lz4_flex 0.11.6: a 26-byte input claiming 3 GB really
        // does reserve 3 GB and then return `Ok`. A tile's own real
        // maximum output is fixed and known, so anything past it is
        // refused before `lz4_flex` is handed the bytes at all.
        let texels = vec![f16::from_f32(0.25); crate::SAMPLES];
        let mut encoded = encode(&texels);
        assert!(
            encoded.get(5) == Some(&1),
            "this test needs the compressed path; uniform data must take it"
        );
        // Patch the four-byte little-endian size prefix that follows the
        // 8-byte header.
        let Some(prefix) = encoded.get_mut(8..12) else {
            unreachable!("a compressed payload always carries its own 4-byte size prefix");
        };
        prefix.copy_from_slice(&3_000_000_000u32.to_le_bytes());

        match decode(&encoded) {
            Err(crate::TileError::CorruptFile(message)) => {
                assert!(
                    message.contains("3000000000"),
                    "the rejection must name the claimed size: {message}"
                );
            }
            other => unreachable!("expected CorruptFile, got {other:?}"),
        }
    }

    #[test]
    fn accepts_a_compressed_payload_claiming_exactly_one_whole_tile() {
        // The other side of the same check: a full tile is the largest
        // legitimate payload there is, so it must not be what gets
        // rejected.
        let texels = vec![f16::from_f32(0.5); crate::SAMPLES];
        let encoded = encode(&texels);
        let decoded = match decode(&encoded) {
            Ok(decoded) => decoded,
            Err(err) => unreachable!("a full tile must still decode: {err}"),
        };
        assert_eq!(decoded.len(), crate::SAMPLES);
    }

    #[test]
    fn rejects_a_compressed_payload_that_decodes_to_less_than_one_whole_tile() {
        // The exact shape scratch-disk corruption takes: a *well-formed*
        // ATIL file -- right magic, right version, valid lz4 frame, a size
        // prefix well inside any upper bound -- that simply holds part of
        // a tile. Before the exact-length check this decoded `Ok` into a
        // short `Vec<f16>`, which `aurora-app`'s own `write_composited`
        // then handed to `copy_from_slice` (a panic) and
        // `aurora_render::composite_layer_into` silently half-composited.
        //
        // A *quarter* tile, not a half: half a tile is 262144 bytes,
        // which is also `SAMPLES`' own numeral, so a message naming the
        // expected sample count would satisfy a bare `contains("262144")`
        // whether or not the actual decoded size were reported at all. A
        // quarter tile's 131072 collides with nothing.
        let quarter = vec![f16::from_f32(0.5); crate::SAMPLES / 4];
        let encoded = encode_any_length(&quarter);
        assert!(
            encoded.get(5) == Some(&1),
            "this test needs the compressed path; uniform data must take it"
        );
        match decode(&encoded) {
            Err(crate::TileError::CorruptFile(message)) => {
                assert!(
                    message.contains("claims 131072 decompressed bytes")
                        && message.contains("524288"),
                    "the rejection must name both the actual and the expected byte counts: {message}"
                );
            }
            other => unreachable!("expected CorruptFile, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_raw_payload_that_is_not_exactly_one_whole_tile() {
        // The `compressed: 0` fallback branch of the same check --
        // unreachable through `encode` (it only emits a raw payload when
        // lz4 expands the data), so the header is built by hand here.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"ATIL");
        bytes.push(1); // version
        bytes.push(0); // raw, not compressed
        bytes.extend_from_slice(&[0, 0]); // reserved
        // Four f16 samples: a whole number of samples, nowhere near a
        // whole tile.
        bytes.extend_from_slice(&[0u8; 8]);
        match decode(&bytes) {
            Err(crate::TileError::CorruptFile(message)) => {
                // The message, not a bare `CorruptFile(_)`: that wildcard
                // also matches a bad-magic or bad-version rejection, so a
                // change that broke header parsing earlier in `decode`
                // would leave this test green while proving nothing about
                // the length check it is named for.
                assert!(
                    message.contains("payload decoded to 8 bytes") && message.contains("524288"),
                    "the rejection must come from the exact-length check, naming both counts: \
                     {message}"
                );
            }
            other => unreachable!("expected CorruptFile, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_raw_payload_of_odd_length() {
        // The property the old `is_multiple_of(2)` check owned, now covered
        // by the strict equality check that replaced it (an odd length is
        // never equal to an even constant) -- asserted so the replacement is
        // proven, not assumed.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"ATIL");
        bytes.push(1);
        bytes.push(0);
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&[0u8; 7]);
        match decode(&bytes) {
            Err(crate::TileError::CorruptFile(message)) => {
                // Same reason as the sibling test above: assert the
                // rejection is the length check's, not the header's.
                assert!(
                    message.contains("payload decoded to 7 bytes") && message.contains("524288"),
                    "the rejection must come from the exact-length check, naming both counts: \
                     {message}"
                );
            }
            other => unreachable!("expected CorruptFile, got {other:?}"),
        }
    }
}
