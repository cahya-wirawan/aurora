//! Tests for the global pattern blocks (task 111): synthetic blocks built
//! byte by byte, hostile variants, and the corpus fixtures.
#![allow(
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value
)]

use std::collections::BTreeSet;
use std::io::Write as _;

use super::pattern::{PatternPool, read_pool};
use super::{StreamSource, Window};

fn u32b(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}

/// One written channel record: `(depth, compression, data)`; `None` is an
/// unwritten one.
#[derive(Clone)]
struct Chan {
    pixel_depth: u16,
    compression: u8,
    data: Vec<u8>,
    rect: Option<[u32; 4]>,
}

fn raw8(samples: &[u8]) -> Chan {
    Chan {
        pixel_depth: 8,
        compression: 0,
        data: samples.to_vec(),
        rect: None,
    }
}

fn raw16(samples: &[u16]) -> Chan {
    Chan {
        pixel_depth: 16,
        compression: 0,
        data: samples.iter().flat_map(|v| v.to_be_bytes()).collect(),
        rect: None,
    }
}

/// A pattern item body (no length prefix).
fn item(id: &str, mode: u32, w: u32, h: u32, channels: &[Chan], extra_records: u32) -> Vec<u8> {
    let mut o = Vec::new();
    u32b(&mut o, 1);
    u32b(&mut o, mode);
    o.extend_from_slice(&[0, 0, 0, 0]);
    let name: Vec<u16> = "pat".encode_utf16().collect();
    u32b(&mut o, name.len() as u32 + 1);
    for u in name {
        o.extend_from_slice(&u.to_be_bytes());
    }
    o.extend_from_slice(&[0, 0]);
    o.push(id.len() as u8);
    o.extend_from_slice(id.as_bytes());
    // The virtual memory array list.
    u32b(&mut o, 3);
    let mut body = Vec::new();
    for v in [0, 0, h, w] {
        u32b(&mut body, v);
    }
    let total = channels.len() as u32 + extra_records;
    u32b(&mut body, total.saturating_sub(2));
    for ch in channels {
        u32b(&mut body, 1);
        u32b(&mut body, 23 + ch.data.len() as u32);
        u32b(&mut body, u32::from(ch.pixel_depth));
        for v in ch.rect.unwrap_or([0, 0, h, w]) {
            u32b(&mut body, v);
        }
        body.extend_from_slice(&ch.pixel_depth.to_be_bytes());
        body.push(ch.compression);
        body.extend_from_slice(&ch.data);
    }
    for _ in 0..extra_records {
        u32b(&mut body, 0);
    }
    u32b(&mut o, body.len() as u32);
    o.extend_from_slice(&body);
    o
}

fn rgb_item(id: &str, w: u32, h: u32, r: &[u8], g: &[u8], b: &[u8]) -> Vec<u8> {
    item(id, 3, w, h, &[raw8(r), raw8(g), raw8(b)], 23)
}

/// Items framed as a block: length, body, padding to four.
fn block(items: &[Vec<u8>]) -> Vec<u8> {
    let mut o = Vec::new();
    for it in items {
        u32b(&mut o, it.len() as u32);
        o.extend_from_slice(it);
        while o.len() % 4 != 0 {
            o.push(0);
        }
    }
    o
}

fn read_with(bytes: Vec<u8>, ids: &[&str]) -> PatternPool {
    let wanted: BTreeSet<String> = ids.iter().map(|s| (*s).to_owned()).collect();
    let len = bytes.len() as u64;
    let mut src = match StreamSource::new(std::io::Cursor::new(bytes)) {
        Ok(s) => s,
        Err(err) => unreachable!("{err:?}"),
    };
    let mut windows = [Window { pos: 0, end: len }];
    match read_pool(&mut src, &mut windows, &wanted) {
        Ok(pool) => pool,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn texels(pool: &PatternPool, id: &str) -> Vec<u16> {
    pool.ready
        .get(id)
        .map(|p| p.texels().to_vec())
        .unwrap_or_default()
}

#[test]
fn an_rgb_8_bit_pattern_decodes_exactly_and_promotes_by_257() {
    let bytes = block(&[rgb_item("a-1", 2, 2, &[0, 1, 2, 255], &[10, 20, 30, 40], &[7, 7, 7, 7])]);
    let pool = read_with(bytes, &["a-1"]);
    let p = pool.ready.get("a-1").map(|p| (p.width(), p.height()));
    assert_eq!(p, Some((2, 2)));
    let t = texels(&pool, "a-1");
    assert_eq!(&t[..4], &[0, 10 * 257, 7 * 257, 65_535]);
    assert_eq!(&t[4..8], &[257, 20 * 257, 7 * 257, 65_535]);
    assert_eq!(&t[12..16], &[65_535, 40 * 257, 7 * 257, 65_535]);
    assert!(pool.refused.is_empty());
}

#[test]
fn a_fourth_written_channel_is_alpha_and_a_grey_pattern_expands() {
    let with_alpha = item(
        "al",
        3,
        2,
        1,
        &[raw8(&[1, 2]), raw8(&[3, 4]), raw8(&[5, 6]), raw8(&[0, 255])],
        22,
    );
    let grey = item("gr", 1, 2, 1, &[raw8(&[9, 200])], 25);
    let pool = read_with(block(&[with_alpha, grey]), &["al", "gr"]);
    assert_eq!(texels(&pool, "al")[3], 0);
    assert_eq!(texels(&pool, "al")[7], 65_535);
    assert_eq!(
        texels(&pool, "gr"),
        vec![9 * 257, 9 * 257, 9 * 257, 65_535, 200 * 257, 200 * 257, 200 * 257, 65_535]
    );
}

#[test]
fn sixteen_bit_samples_are_kept_as_they_are() {
    let it = item(
        "d16",
        3,
        2,
        1,
        &[raw16(&[1, 65_535]), raw16(&[300, 2]), raw16(&[9, 40_000])],
        23,
    );
    let pool = read_with(block(&[it]), &["d16"]);
    assert_eq!(texels(&pool, "d16"), vec![1, 300, 9, 65_535, 65_535, 2, 40_000, 65_535]);
}

#[test]
fn rle_zip_and_zip_with_prediction_decode_to_the_same_pixels() {
    let plane: Vec<u8> = (0..12u8).map(|i| i.wrapping_mul(21)).collect(); // 4 x 3
    // PackBits, literal runs only: a header of n-1 then n bytes, per row.
    let mut rle = Vec::new();
    let mut rows = Vec::new();
    for row in plane.chunks(4) {
        let mut r = vec![3u8];
        r.extend_from_slice(row);
        rows.push(r);
    }
    for r in &rows {
        rle.extend_from_slice(&(r.len() as u16).to_be_bytes());
    }
    for r in &rows {
        rle.extend_from_slice(r);
    }
    let deflate = |data: &[u8]| {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = z.write_all(data);
        z.finish().unwrap_or_default()
    };
    let mut delta = plane.clone();
    for row in delta.chunks_mut(4) {
        for i in (1..row.len()).rev() {
            row[i] = row[i].wrapping_sub(row[i - 1]);
        }
    }
    let chan = |compression: u8, data: Vec<u8>| Chan {
        pixel_depth: 8,
        compression,
        data,
        rect: None,
    };
    let items: Vec<Vec<u8>> = [
        ("raw", 0u8, plane.clone()),
        ("rle", 1, rle),
        ("zip", 2, deflate(&plane)),
        ("pre", 3, deflate(&delta)),
    ]
    .into_iter()
    .map(|(id, comp, data)| {
        let c = chan(comp, data);
        item(id, 3, 4, 3, &[c.clone(), c.clone(), c], 23)
    })
    .collect();
    let pool = read_with(block(&items), &["raw", "rle", "zip", "pre"]);
    let want = texels(&pool, "raw");
    assert_eq!(want.len(), 48);
    for id in ["rle", "zip", "pre"] {
        assert_eq!(texels(&pool, id), want, "{id}");
    }
    assert!(pool.refused.is_empty());
}

#[test]
fn unwanted_items_are_ignored_and_the_first_copy_of_an_id_wins() {
    let a = rgb_item("one", 1, 1, &[10], &[10], &[10]);
    let b = rgb_item("two", 1, 1, &[20], &[20], &[20]);
    let again = rgb_item("one", 1, 1, &[99], &[99], &[99]);
    let pool = read_with(block(&[a, b, again]), &["one"]);
    assert_eq!(pool.ready.len(), 1);
    assert_eq!(texels(&pool, "one")[0], 10 * 257);
    assert!(pool.refused.is_empty(), "'two' was never asked for");
    // Nothing asked for: nothing read.
    let pool = read_with(block(&[rgb_item("x", 1, 1, &[1], &[1], &[1])]), &[]);
    assert!(pool.ready.is_empty() && pool.refused.is_empty());
}

#[test]
fn every_unsupported_or_damaged_pattern_is_refused_by_name() {
    let ok = |id: &str| rgb_item(id, 2, 1, &[1, 2], &[3, 4], &[5, 6]);
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    // Indexed, CMYK, Lab, Bitmap.
    for (name, mode) in [("idx", 2u32), ("cmyk", 4), ("lab", 9), ("bmp", 0)] {
        cases.push((name, item(name, mode, 2, 1, &[raw8(&[1, 2]), raw8(&[1, 2]), raw8(&[1, 2])], 23)));
    }
    // 32-bit and 1-bit depth.
    let mut d32 = raw8(&[0; 8]);
    d32.pixel_depth = 32;
    cases.push(("d32", item("d32", 3, 2, 1, &[d32.clone(), d32.clone(), d32], 23)));
    let mut d1 = raw8(&[0]);
    d1.pixel_depth = 1;
    cases.push(("d1", item("d1", 3, 2, 1, &[d1.clone(), d1.clone(), d1], 23)));
    // Unknown compression.
    let mut c9 = raw8(&[1, 2]);
    c9.compression = 9;
    cases.push(("c9", item("c9", 3, 2, 1, &[c9.clone(), c9.clone(), c9], 23)));
    // Too few written colour channels; a short plane; a mismatched rectangle.
    cases.push(("few", item("few", 3, 2, 1, &[raw8(&[1, 2]), raw8(&[1, 2])], 24)));
    cases.push(("short", item("short", 3, 2, 1, &[raw8(&[1]), raw8(&[1, 2]), raw8(&[1, 2])], 23)));
    let mut odd = raw8(&[1, 2]);
    odd.rect = Some([0, 0, 1, 3]);
    cases.push(("rect", item("rect", 3, 2, 1, &[odd.clone(), odd.clone(), odd], 23)));
    // A bad zlib stream, a bad PackBits row.
    let mut z = raw8(&[1, 2, 3]);
    z.compression = 2;
    cases.push(("zbad", item("zbad", 3, 2, 1, &[z.clone(), z.clone(), z], 23)));
    let mut r = raw8(&[0, 1, 0, 0]);
    r.compression = 1;
    cases.push(("rbad", item("rbad", 3, 2, 1, &[r.clone(), r.clone(), r], 23)));
    // Huge and degenerate sizes are refused on the size alone: the data is
    // one byte, so any allocation from the claim would be the bug.
    for (name, w, h) in [("huge", 60_000, 60_000), ("zero", 0, 5), ("wide", 9_000, 1), ("big", 3_000, 3_000)] {
        cases.push((name, item(name, 3, w, h, &[raw8(&[1]), raw8(&[1]), raw8(&[1])], 23)));
    }
    // More channel records than any pattern needs.
    cases.push(("many", item("many", 3, 2, 1, &[raw8(&[1, 2]), raw8(&[1, 2]), raw8(&[1, 2])], 400)));
    let ids: Vec<&str> = cases.iter().map(|(n, _)| *n).chain(["good"]).collect();
    let mut items: Vec<Vec<u8>> = cases.into_iter().map(|(_, b)| b).collect();
    items.push(ok("good"));
    let pool = read_with(block(&items), &ids);
    assert_eq!(pool.ready.keys().collect::<Vec<_>>(), vec!["good"]);
    let refused: Vec<&str> = pool.refused.iter().map(String::as_str).collect();
    for name in ids.iter().filter(|n| **n != "good") {
        assert!(refused.contains(name), "{name} not refused: {refused:?}");
    }
}

#[test]
fn a_pool_past_its_byte_cap_stops_at_the_cap() {
    // Eleven 2048 x 2048 patterns would be 88 MiB of u16 RGBA; the pool
    // holds 64 MiB. (The planes are one byte: only the size claim matters,
    // so each is refused on its data, but the cap bookkeeping is by size.)
    let one = |id: String| {
        let n = 2048 * 2048;
        let plane = vec![7u8; n];
        item(&id, 3, 2048, 2048, &[raw8(&plane), raw8(&plane), raw8(&plane)], 23)
    };
    let ids: Vec<String> = (0..9).map(|i| format!("p{i}")).collect();
    let items: Vec<Vec<u8>> = ids.iter().map(|id| one(id.clone())).collect();
    let wanted: Vec<&str> = ids.iter().map(String::as_str).collect();
    let pool = read_with(block(&items), &wanted);
    // 32 MiB each: two fit in 64 MiB, the rest are refused.
    assert_eq!(pool.ready.len(), 2, "{:?}", pool.ready.keys());
    assert_eq!(pool.refused.len(), 7);
}

#[test]
fn a_damaged_block_never_panics_at_any_truncation_or_byte_flip() {
    let good = block(&[
        rgb_item("alpha", 3, 2, &[1, 2, 3, 4, 5, 6], &[6, 5, 4, 3, 2, 1], &[9; 6]),
        item("beta", 1, 2, 2, &[raw8(&[1, 2, 3, 4])], 25),
    ]);
    // Every prefix.
    for n in 0..good.len() {
        let _ = read_with(good[..n].to_vec(), &["alpha", "beta"]);
    }
    // Deterministic mutations (xorshift).
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..3000 {
        let mut b = good.clone();
        for _ in 0..1 + (next() % 4) {
            let at = (next() % b.len() as u64) as usize;
            b[at] = next() as u8;
        }
        let _ = read_with(b, &["alpha", "beta"]);
    }
    // Pure noise, and lengths that claim more than the window holds.
    for len in [0usize, 3, 4, 7, 64, 1000] {
        let noise: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let _ = read_with(noise, &["alpha"]);
    }
    let mut huge = Vec::new();
    u32b(&mut huge, u32::MAX);
    huge.extend_from_slice(&[0; 32]);
    let pool = read_with(huge, &["alpha"]);
    assert!(pool.ready.is_empty());
}

fn corpus(name: &str) -> Option<Vec<u8>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures")
        .join(name);
    let bytes = std::fs::read(&path).ok();
    if bytes.is_none() {
        println!("SKIPPED: corpus file not present at {}", path.display());
    }
    bytes
}

#[test]
fn patterns_psd_yields_its_six_patterns_with_psd_tools_pixels() {
    let Some(bytes) = corpus("patterns.psd") else {
        return;
    };
    let file = match super::decode(&bytes) {
        Ok(f) => f,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(file.patterns.len(), 6);
    let by_id = |id: &str| file.patterns.iter().find(|p| p.id() == id);
    // 'Right Diagonal Line 1', 8 x 8: row 0 red channel read with
    // psd-tools 1.17.4 is [220 255 255 255 255 255 223 56].
    let Some(diag) = by_id("bf565312-67b6-1177-9181-9d5762aa7056") else {
        unreachable!("diagonal pattern missing");
    };
    assert_eq!((diag.width(), diag.height()), (8, 8));
    let red: Vec<u16> = diag.texels().chunks(4).take(8).map(|t| t[0] / 257).collect();
    assert_eq!(red, vec![220, 255, 255, 255, 255, 255, 223, 56]);
    // A one-texel-wide pattern and the RLE-compressed Water pattern.
    let Some(v) = by_id("777f73b8-688d-1177-b2e5-b715d4e2a635") else {
        unreachable!("vertical line pattern missing");
    };
    // psd-tools' rectangle (top, left, bottom, right) = (0, 0, 1, 4): 4 wide.
    assert_eq!((v.width(), v.height()), (4, 1));
    let Some(water) = by_id("2ab8ff7f-7c39-11d4-97f0-ce0e81abb313") else {
        unreachable!("water pattern missing");
    };
    assert_eq!((water.width(), water.height()), (80, 80));
    assert!(water.texels().chunks(4).all(|t| t[3] == 65_535));
    assert_eq!(by_id("b2fdfd29-de85-11d5-838b-ff55e75fb875").map(|p| (p.width(), p.height())), Some((219, 265)));
}

#[test]
fn only_the_patterns_a_layer_names_are_kept() {
    // fill_adjustments.psd's block holds one pattern (Yellow Mums), named
    // by its enabled overlay on Rectangle 2 and its disabled one on 3.
    let Some(bytes) = corpus("fill_adjustments.psd") else {
        return;
    };
    let file = match super::decode(&bytes) {
        Ok(f) => f,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(file.patterns.len(), 1);
    // A file whose layers name no pattern keeps none, though its block
    // holds one (adjustment-fillers.psd has a pattern fill layer only).
    let Some(bytes) = corpus("adjustment-fillers.psd") else {
        return;
    };
    let file = match super::decode(&bytes) {
        Ok(f) => f,
        Err(err) => unreachable!("{err:?}"),
    };
    assert!(file.patterns.is_empty());
}
