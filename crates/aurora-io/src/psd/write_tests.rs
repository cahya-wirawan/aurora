//! Tests for the PSD writer (0.176.0, round R1; groups and masks 0.178.0, R3).
#![allow(clippy::many_single_char_names)]

use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::Path;

use aurora_color::{IccProfile, quantize_u16};
use aurora_core::Rect;
use aurora_doc::{Adjustment, BlendMode, LayerId, LayerTree};
use aurora_tile::TileStore;
use half::f16;

use super::test_writer::{TestLayer, TestPsd};
use super::write::{
    PsdCompositeSource, PsdExpectation, PsdSaveOptions, PsdSaveRefusal, PsdWriteOutcome,
    psd_blend_key, psd_save_preflight, verify_psd_file, write_psd,
};
use super::{PsdBlend, blend_for_key, unpack_bits};
use crate::error::IoError;
use crate::image::Image;
use crate::replace_verified;

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = NonZeroUsize::new(8) else {
        unreachable!("8 is non-zero");
    };
    let store = ok(TileStore::new(dir.path().to_path_buf(), budget));
    (dir, store)
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> [f32; 4]) -> Image {
    let mut samples = Vec::new();
    for y in 0..height {
        for x in 0..width {
            samples.extend(f(x, y).map(f16::from_f32));
        }
    }
    ok(Image::new(width, height, IccProfile::srgb(), samples))
}

/// A test document: layers added bottom first.
struct Doc {
    tree: LayerTree,
    _dir: tempfile::TempDir,
    store: TileStore,
    canvas: (u32, u32),
    composite: Image,
}

impl Doc {
    fn new(canvas: (u32, u32)) -> Self {
        let (dir, store) = store();
        Self {
            tree: LayerTree::new(),
            _dir: dir,
            store,
            canvas,
            composite: image(canvas.0, canvas.1, |x, y| {
                [x as f32 / 64.0, y as f32 / 64.0, 0.5, 1.0]
            }),
        }
    }

    /// Adds a layer on top with `bounds`, its pixels at surface offset
    /// `(dx, dy)`.
    fn layer(&mut self, name: &str, bounds: Rect, pixels: Option<(&Image, u32, u32)>) -> LayerId {
        let id = ok(self.tree.add_pixel_layer(name, bounds, None));
        if let Some((img, dx, dy)) = pixels {
            let Some(surface) = self.tree.surface_id(id) else {
                unreachable!("pixel layer");
            };
            ok(crate::write_into_store_at(
                img,
                &mut self.store,
                surface,
                dx,
                dy,
            ));
        }
        id
    }

    fn write(&mut self) -> Result<(Vec<u8>, PsdWriteOutcome), IoError> {
        let mut out = Cursor::new(Vec::new());
        let mut source = &self.composite;
        let outcome = write_psd(
            &mut out,
            &self.tree,
            self.canvas,
            &mut source,
            &mut self.store,
            PsdSaveOptions::default(),
        )?;
        Ok((out.into_inner(), outcome))
    }
}

/// Exact `f16` values that survive `f16` -> `u16` -> `f16`.
const LEVELS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

fn level(i: u32) -> f32 {
    LEVELS.get((i % 5) as usize).copied().unwrap_or(0.0)
}

fn be16(bytes: &[u8], at: usize) -> u16 {
    match bytes.get(at..at + 2) {
        Some([a, b]) => u16::from_be_bytes([*a, *b]),
        _ => unreachable!("short file"),
    }
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    match bytes.get(at..at + 4) {
        Some([a, b, c, d]) => u32::from_be_bytes([*a, *b, *c, *d]),
        _ => unreachable!("short file"),
    }
}

fn byte(bytes: &[u8], at: usize) -> u8 {
    bytes
        .get(at)
        .copied()
        .unwrap_or_else(|| unreachable!("short file"))
}

/// Offset of the layer-and-mask section's length: header 26, colour data
/// 4, then the image resources (0.177.0: the sRGB ICC profile) behind
/// their own 4-byte length.
fn section_at(bytes: &[u8]) -> usize {
    26 + 4 + 4 + be32(bytes, 30) as usize
}

/// Offset of the first layer record: the section length 4, layer info
/// length 4, global mask 4, `8BIM` `Lr16` and its length 12, layer
/// count 2.
fn first_record(bytes: &[u8]) -> usize {
    section_at(bytes) + 4 + 4 + 4 + 12 + 2
}

/// One parsed record: (rect, [(id, len)], key, opacity, flags, extra
/// start, extra len), and where the next one starts.
struct ParsedRecord {
    rect: [i32; 4],
    channels: Vec<(i16, u32)>,
    flags: u8,
    extra_at: usize,
    end: usize,
}

fn parse_record(bytes: &[u8], at: usize) -> ParsedRecord {
    let edge = |i: usize| be32(bytes, at + 4 * i).cast_signed();
    let rect = [edge(0), edge(1), edge(2), edge(3)];
    let n = usize::from(be16(bytes, at + 16));
    let channels = (0..n)
        .map(|i| {
            let c = at + 18 + 6 * i;
            (be16(bytes, c).cast_signed(), be32(bytes, c + 2))
        })
        .collect();
    let blend_at = at + 18 + 6 * n;
    let flags = byte(bytes, blend_at + 10);
    let extra_len = be32(bytes, blend_at + 12) as usize;
    let extra_at = blend_at + 16;
    ParsedRecord {
        rect,
        channels,
        flags,
        extra_at,
        end: extra_at + extra_len,
    }
}

fn read_back(bytes: &[u8]) -> super::PsdDocument {
    ok(super::read(bytes))
}

/// Root ids bottom-to-top.
fn bottom_up(tree: &LayerTree) -> Vec<LayerId> {
    tree.roots().iter().rev().copied().collect()
}

#[test]
fn write_psd_round_trips_flat_layers_through_read() {
    let mut doc = Doc::new((40, 30));
    let a = image(40, 30, |x, y| [level(x), level(y), level(x + y), 1.0]);
    let b = image(10, 6, |x, y| [level(y), 1.0, level(x), level(x * y)]);
    let bottom = doc.layer("Background", rect(0, 0, 40, 30), Some((&a, 0, 0)));
    let top = doc.layer("Top", rect(5, 7, 12, 9), Some((&b, 2, 3)));
    let empty = doc.layer("Empty", rect(0, 0, 40, 30), None);
    ok(doc.tree.set_opacity(top, 128.0 / 255.0));
    ok(doc.tree.set_fill_opacity(top, 64.0 / 255.0));
    ok(doc.tree.set_blend_mode(top, BlendMode::Multiply));
    ok(doc.tree.set_visible(empty, false));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);

    let back = read_back(&bytes);
    assert_eq!(back.canvas_size, (40, 30));
    let ids = bottom_up(&back.layers);
    let names: Vec<_> = ids.iter().map(|&id| back.layers.name(id)).collect();
    assert_eq!(names, [Some("Background"), Some("Top"), Some("Empty")]);
    let t = ids.get(1).copied().unwrap_or_else(|| unreachable!());
    assert_eq!(back.layers.opacity(t), Some(128.0 / 255.0));
    assert_eq!(back.layers.fill_opacity(t), Some(64.0 / 255.0));
    assert_eq!(back.layers.blend_mode(t), Some(BlendMode::Multiply));
    let e = ids.get(2).copied().unwrap_or_else(|| unreachable!());
    assert_eq!(back.layers.visible(e), Some(false));
    assert_eq!(back.layers.visible(t), Some(true));
    // The top layer's record is its pixels' tile-occupied extent: the
    // whole 12x9 bounds at (5, 7).
    let tp = back.pixels.iter().find(|p| p.layer == t);
    let Some(tp) = tp else {
        unreachable!("top has pixels")
    };
    assert_eq!((tp.image.width(), tp.image.height()), (12, 9));
    // Its own pixels, exactly, at (2, 3) inside; zeros elsewhere.
    for y in 0..9u32 {
        for x in 0..12u32 {
            let want = if (2..12).contains(&x) && (3..9).contains(&y) {
                let (bx, by) = (x - 2, y - 3);
                [level(by), 1.0, level(bx), level(bx * by)]
            } else {
                [0.0; 4]
            };
            let i = ((y * 12 + x) * 4) as usize;
            let got: Vec<f32> = (0..4)
                .map(|c| tp.image.samples().get(i + c).map_or(-1.0, |v| v.to_f32()))
                .collect();
            assert_eq!(got, want, "({x}, {y})");
        }
    }
    let bp = back
        .pixels
        .iter()
        .find(|p| Some(p.layer) == ids.first().copied());
    let Some(bp) = bp else { unreachable!() };
    assert_eq!(bp.image.samples(), a.samples());
    // The empty layer has no pixels and an empty rectangle.
    assert!(!back.pixels.iter().any(|p| p.layer == e));
    let _ = bottom;

    // And the writer's own verification accepts it.
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("flat.psd");
    ok(std::fs::write(&path, &bytes));
    ok(verify_psd_file(&path, &outcome.expected));
}

#[test]
fn write_psd_all_27_blend_keys_round_trip() {
    for mode in BlendMode::modes_for(false) {
        assert_eq!(blend_for_key(psd_blend_key(mode)), PsdBlend::Mode(mode));
    }
    // 0.179.0: Pass Through is written as `pass`, which the reader keeps
    // for a group (`pass_through_group_writes_pass_and_round_trips`).
    assert_eq!(psd_blend_key(BlendMode::PassThrough), *b"pass");
    assert_eq!(
        blend_for_key(psd_blend_key(BlendMode::PassThrough)),
        PsdBlend::PassThrough
    );
    let mut keys: Vec<[u8; 4]> = BlendMode::ALL.iter().map(|&m| psd_blend_key(m)).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 28);

    let mut doc = Doc::new((4, 4));
    let px = image(4, 4, |_, _| [0.5, 0.25, 0.75, 1.0]);
    for mode in BlendMode::modes_for(false) {
        let id = doc.layer(&format!("{mode:?}"), rect(0, 0, 4, 4), Some((&px, 0, 0)));
        ok(doc.tree.set_blend_mode(id, mode));
    }
    let (bytes, _) = ok(doc.write());
    let back = read_back(&bytes);
    let modes: Vec<_> = bottom_up(&back.layers)
        .iter()
        .map(|&id| back.layers.blend_mode(id))
        .collect();
    let want: Vec<_> = BlendMode::modes_for(false)
        .iter()
        .map(|&m| Some(m))
        .collect();
    assert_eq!(modes, want);
}

/// AC-3 (0.179.0): a Pass Through group is written as `pass` in both its
/// record and its `lsct`, reads back as Pass Through with nothing
/// reported, and passes the writer's own verification; an isolated
/// Normal group beside it still writes `norm`.
#[test]
fn pass_through_group_writes_pass_and_round_trips() {
    let mut doc = Doc::new((4, 4));
    doc.layer_in("Base", rect(0, 0, 4, 4), Some(&px(4, 4, 0)), None);
    let g = doc.group("Pass", None);
    let inner = doc.layer_in("In", rect(0, 0, 4, 4), Some(&px(4, 4, 1)), Some(g));
    ok(doc.tree.set_blend_mode(inner, BlendMode::Multiply));
    ok(doc.tree.set_blend_mode(g, BlendMode::PassThrough));
    ok(doc.tree.set_opacity(g, 128.0 / 255.0));
    let normal = doc.group("Plain", None);
    doc.layer_in("In2", rect(0, 0, 2, 2), Some(&px(2, 2, 2)), Some(normal));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    // Records: Base, end, In, Pass, end, In2, Plain.
    assert_eq!(blend_key_of(&bytes, 3), *b"pass");
    assert_eq!(blend_key_of(&bytes, 6), *b"norm");
    let recs = records(&bytes);
    let mut folder = 1u32.to_be_bytes().to_vec();
    folder.extend_from_slice(b"8BIMpass");
    assert_eq!(rec(&recs, 3).0.as_deref(), Some(folder.as_slice()));
    let back = read_back(&bytes);
    let l = &back.layers;
    let Some(pass) = l
        .roots()
        .iter()
        .copied()
        .find(|&id| l.name(id) == Some("Pass"))
    else {
        unreachable!("group read back")
    };
    assert_eq!(l.blend_mode(pass), Some(BlendMode::PassThrough));
    assert_eq!(l.opacity(pass), Some(128.0 / 255.0));
    assert!(back.report.items.is_empty(), "{:?}", back.report);
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("pass.psd"), false));
}

#[test]
fn hidden_layer_sets_flag_bit_1() {
    for visible in [true, false] {
        let mut doc = Doc::new((4, 4));
        let id = doc.layer("L", rect(0, 0, 4, 4), None);
        ok(doc.tree.set_visible(id, visible));
        let (bytes, _) = ok(doc.write());
        let record = parse_record(&bytes, first_record(&bytes));
        assert_eq!(record.flags & 0x02 != 0, !visible);
        assert_eq!(record.flags & !0x02, 0);
    }
}

#[test]
fn unicode_name_writes_luni_and_ascii_pascal() {
    let name = "Ébauche 草稿 🎨";
    let mut doc = Doc::new((4, 4));
    doc.layer(name, rect(0, 0, 4, 4), None);
    let (bytes, _) = ok(doc.write());
    let record = parse_record(&bytes, first_record(&bytes));
    // Mask length and blending-ranges length, both 0, then the name.
    let pascal_at = record.extra_at + 8;
    let len = usize::from(byte(&bytes, pascal_at));
    let pascal = bytes.get(pascal_at + 1..pascal_at + 1 + len);
    assert_eq!(pascal, Some(&b"?bauche ?? ?"[..]));
    // Padded to 4, the length byte included: `luni` starts right after.
    let padded = pascal_at + (1 + len).next_multiple_of(4);
    assert_eq!(bytes.get(padded..padded + 8), Some(&b"8BIMluni"[..]));
    let mut luni = b"8BIMluni".to_vec();
    let units: Vec<u16> = name.encode_utf16().collect();
    luni.extend_from_slice(&(4 + 2 * units.len() as u32).to_be_bytes());
    luni.extend_from_slice(&(units.len() as u32).to_be_bytes());
    for u in &units {
        luni.extend_from_slice(&u.to_be_bytes());
    }
    let extra = bytes.get(record.extra_at..record.end).unwrap_or_default();
    assert!(extra.windows(luni.len()).any(|w| w == luni.as_slice()));
    let back = read_back(&bytes);
    let id = back
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    assert_eq!(back.layers.name(id), Some(name));
}

#[test]
fn long_names_are_shortened_and_itemised() {
    let long: String = "名".repeat(300);
    let mut doc = Doc::new((4, 4));
    doc.layer(&long, rect(0, 0, 4, 4), None);
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(
        outcome.report.items,
        ["1 layer name is longer than 255 characters will be shortened."]
    );
    let back = read_back(&bytes);
    let id = back
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    assert_eq!(back.layers.name(id).map(|n| n.chars().count()), Some(255));
}

/// Walks every channel of every record (bottom-to-top) and returns, per
/// channel, `(declared length, compression, row counts, data start)`.
fn channel_layout(bytes: &[u8], layers: usize) -> Vec<(u32, u16, Vec<u16>, usize, usize)> {
    let mut at = first_record(bytes);
    let mut records = Vec::new();
    for _ in 0..layers {
        let r = parse_record(bytes, at);
        at = r.end;
        records.push(r);
    }
    let mut out = Vec::new();
    for r in &records {
        let height = usize::try_from(r.rect[2] - r.rect[0]).unwrap_or(0);
        for &(_, len) in &r.channels {
            let compression = be16(bytes, at);
            let counts: Vec<u16> = (0..height).map(|i| be16(bytes, at + 2 + 2 * i)).collect();
            out.push((len, compression, counts, at + 2 + 2 * height, at));
            at += len as usize;
        }
    }
    out
}

#[test]
fn channel_length_includes_compression_field() {
    let mut doc = Doc::new((9, 5));
    let px = image(9, 5, |x, y| [level(x), level(y), 0.5, 1.0]);
    doc.layer("A", rect(0, 0, 9, 5), Some((&px, 0, 0)));
    doc.layer("Empty", rect(0, 0, 9, 5), None);
    let (bytes, _) = ok(doc.write());
    let layout = channel_layout(&bytes, 2);
    assert_eq!(layout.len(), 8);
    for (len, compression, counts, data_at, start) in &layout {
        assert_eq!(*compression, 1);
        let rows: usize = counts.iter().map(|&c| usize::from(c)).sum();
        assert_eq!(*len as usize, 2 + 2 * counts.len() + rows);
        assert_eq!(*data_at + rows, *start + *len as usize);
    }
    // An empty layer's channels are exactly the compression field.
    assert!(layout.iter().skip(4).all(|(len, ..)| *len == 2));
}

#[test]
fn rle_row_counts_match_packed_rows() {
    let mut doc = Doc::new((300, 7));
    // Long runs and literals both, across a tile boundary.
    let px = image(300, 7, |x, y| {
        let v = if x < 140 { 0.5 } else { level(x * 7 + y) };
        [v, 1.0 - v, level(x / 3), 1.0]
    });
    doc.layer("A", rect(0, 0, 300, 7), Some((&px, 0, 0)));
    let (bytes, _) = ok(doc.write());
    for (_, _, counts, data_at, _) in channel_layout(&bytes, 1) {
        let mut at = data_at;
        for count in counts {
            let packed = bytes.get(at..at + usize::from(count)).unwrap_or_default();
            let mut row = Vec::new();
            ok(unpack_bits(packed, &mut row, 600));
            assert_eq!(row.len(), 600);
            at += usize::from(count);
        }
    }
}

#[test]
fn psd16_quantize_is_deterministic_and_undithered() {
    let build = || {
        let mut doc = Doc::new((64, 32));
        let px = image(64, 32, |_, _| [0.3, 0.123, 0.9, 0.6]);
        doc.layer("A", rect(0, 0, 64, 32), Some((&px, 0, 0)));
        ok(doc.write()).0
    };
    let first = build();
    assert_eq!(first, build(), "byte-identical across writes");
    // A flat colour stays flat: no dither pattern, every row is one run.
    for (slot, (_, _, counts, data_at, _)) in channel_layout(&first, 1).into_iter().enumerate() {
        let value = [0.6f32, 0.3, 0.123, 0.9]
            .get(slot)
            .copied()
            .unwrap_or_default();
        let q = quantize_u16(f16::from_f32(value).to_f32());
        let mut at = data_at;
        for count in counts {
            let packed = bytes_at(&first, at, usize::from(count));
            let mut row = Vec::new();
            ok(unpack_bits(packed, &mut row, 128));
            assert!(row.chunks_exact(2).all(|p| p == q.to_be_bytes()));
            at += usize::from(count);
        }
    }
}

fn bytes_at(bytes: &[u8], at: usize, n: usize) -> &[u8] {
    bytes.get(at..at + n).unwrap_or_default()
}

#[test]
fn out_of_gamut_and_nan_are_counted() {
    let mut doc = Doc::new((4, 2));
    let px = image(4, 2, |x, y| match (x, y) {
        (0, 0) => [1.5, -0.25, 0.5, 1.0],
        (1, 0) => [f32::NAN, 0.5, 0.5, 1.0],
        (2, 0) => [f32::INFINITY, 0.5, f32::NAN, 1.0],
        _ => [0.5, 0.5, 0.5, 1.0],
    });
    doc.layer("A", rect(0, 0, 4, 2), Some((&px, 0, 0)));
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(
        outcome.report.items,
        [
            "3 colour values outside the 0–1 range will be clipped.",
            "2 colour values that are not a number will be written as 0.",
        ]
    );
    let back = read_back(&bytes);
    let Some(p) = back.pixels.first() else {
        unreachable!()
    };
    let got: Vec<f32> = p
        .image
        .samples()
        .iter()
        .take(12)
        .map(|v| v.to_f32())
        .collect();
    assert_eq!(
        got,
        [1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 0.5, 1.0, 1.0, 0.5, 0.0, 1.0]
    );
}

#[test]
fn merged_image_is_planar_white_matted_and_reads_back() {
    let mut doc = Doc::new((3, 2));
    doc.composite = image(3, 2, |x, _| match x {
        0 => [0.5, 0.0, 1.0, 0.5],
        1 => [0.25, 0.75, 0.0, 1.0],
        _ => [0.0, 0.0, 0.0, 0.0],
    });
    doc.layer("A", rect(0, 0, 3, 2), None);
    let (bytes, _) = ok(doc.write());
    let section = be32(&bytes, section_at(&bytes)) as usize;
    let merged = section_at(&bytes) + 4 + section;
    assert_eq!(be16(&bytes, merged), 1);
    let mut at = merged + 2 + 2 * 8;
    let mut planes = Vec::new();
    for i in 0..8 {
        let count = usize::from(be16(&bytes, merged + 2 + 2 * i));
        let mut row = Vec::new();
        ok(unpack_bits(bytes_at(&bytes, at, count), &mut row, 6));
        planes.push(row.chunks_exact(2).map(|p| be16(p, 0)).collect::<Vec<_>>());
        at += count;
    }
    let q = |v: f32| quantize_u16(v);
    // Rows 0 and 1 of plane 0 (red), then green, blue, alpha: planar.
    let red = vec![q(0.75), q(0.25), q(1.0)];
    let green = vec![q(0.5), q(0.75), q(1.0)];
    let blue = vec![q(1.0), q(0.0), q(1.0)];
    let alpha = vec![q(0.5), q(1.0), q(0.0)];
    let want = [&red, &red, &green, &green, &blue, &blue, &alpha, &alpha];
    for (got, want) in planes.iter().zip(want) {
        assert_eq!(got, want);
    }
    // The reader's merged-image path reads the colour planes back.
    let merged = ok(super::merged_image(&bytes));
    let first: Vec<f32> = merged
        .samples()
        .iter()
        .take(4)
        .map(|v| v.to_f32())
        .collect();
    assert_eq!(first, [0.75, 0.5, 1.0, 1.0]);
}

#[test]
fn writer_matches_test_writer_bytes_for_a_two_layer_file() {
    let mut doc = Doc::new((20, 10));
    let a = image(20, 10, |x, y| [level(x), level(y), 0.5, 1.0]);
    let b = image(8, 5, |x, y| [1.0, level(x + y), 0.0, level(x)]);
    doc.composite = image(20, 10, |x, y| [level(x * y), 0.5, level(y), level(x)]);
    doc.layer("Back", rect(0, 0, 20, 10), Some((&a, 0, 0)));
    let top = doc.layer("Front ü", rect(3, 2, 8, 5), Some((&b, 0, 0)));
    ok(doc.tree.set_opacity(top, 200.0 / 255.0));
    ok(doc.tree.set_fill_opacity(top, 100.0 / 255.0));
    ok(doc.tree.set_blend_mode(top, BlendMode::Screen));
    ok(doc.tree.set_visible(top, false));
    let (bytes, _) = ok(doc.write());

    let rgba = |img: &Image| -> Vec<[u16; 4]> {
        img.samples()
            .chunks_exact(4)
            .map(|p| {
                let v = |c: usize| quantize_u16(p.get(c).map_or(0.0, |s| s.to_f32()));
                [v(0), v(1), v(2), v(3)]
            })
            .collect()
    };
    let layer = |name: &str, l, t, img: &Image| {
        let (w, h) = (img.width().cast_signed(), img.height().cast_signed());
        TestLayer::pixels(name, l, t, w, h, 16, &rgba(img)).with(|l| {
            l.compression = 1;
            l.fill = Some(255);
        })
    };
    let back = layer("Back", 0, 0, &a);
    let front = layer("Front ?", 3, 2, &b).with(|l| {
        l.luni = Some("Front ü".to_owned());
        l.blend = *b"scrn";
        l.opacity = 200;
        l.fill = Some(100);
        l.flags = 0x02;
    });
    let matted: Vec<[u16; 4]> = doc
        .composite
        .samples()
        .chunks_exact(4)
        .map(|p| {
            let s = |c: usize| p.get(c).map_or(0.0, |v| v.to_f32());
            let a = s(3);
            let m = |c: usize| quantize_u16(s(c) * a + 1.0 - a);
            [m(0), m(1), m(2), quantize_u16(a)]
        })
        .collect();
    let planes = (0..4)
        .map(|c| {
            matted
                .iter()
                .flat_map(|px| px.get(c).copied().unwrap_or(0).to_be_bytes())
                .collect()
        })
        .collect();
    let oracle = TestPsd::new(1, 20, 10, 16)
        .with(|f| {
            f.channels = 4;
            f.lr16 = true;
            f.negative_count = true;
            f.layers = vec![back, front];
            f.merged = Some(planes);
            f.merged_compression = 1;
            f.resources = ok(super::write::icc_resource());
        })
        .write();
    assert_eq!(bytes.len(), oracle.len());
    assert!(bytes == oracle, "byte-for-byte equal to the test writer");
}

#[test]
fn preflight_refuses_curves_and_oversize_but_not_groups_or_masks() {
    let opts = PsdSaveOptions::default();
    let base = || {
        let mut tree = LayerTree::new();
        ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
        tree
    };
    let tree = base();
    assert_eq!(
        psd_save_preflight(&tree, (4, 4), opts).map(|r| r.items),
        Ok(vec![])
    );

    // 0.178.0: groups and masks are written.
    let mut groups = base();
    ok(groups.add_group("G", None));
    assert_eq!(
        psd_save_preflight(&groups, (4, 4), opts).map(|r| r.items),
        Ok(vec![])
    );

    let mut masked = base();
    let id = masked
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    ok(masked.add_mask(id, rect(0, 0, 4, 4)));
    assert_eq!(
        psd_save_preflight(&masked, (4, 4), opts).map(|r| r.items),
        Ok(vec![])
    );

    let mut curves = base();
    ok(curves.add_adjustment_layer_at(
        "C",
        Adjustment::Curves(aurora_core::CurvesParams::default()),
        None,
        0,
    ));
    assert_eq!(
        psd_save_preflight(&curves, (4, 4), opts),
        Err(PsdSaveRefusal::CurvesNotYetSupported { count: 1 })
    );

    for canvas in [(30_001, 10), (10, 30_001)] {
        let refusal = psd_save_preflight(&tree, canvas, opts);
        let Err(refusal) = refusal else {
            unreachable!("refused")
        };
        assert!(matches!(refusal, PsdSaveRefusal::TooLargeForPsd { .. }));
        assert!(refusal.to_string().contains("use .psb"), "{refusal}");
    }
    assert!(psd_save_preflight(&tree, (30_000, 30_000), opts).is_ok());
    assert_eq!(
        psd_save_preflight(&LayerTree::new(), (4, 4), opts),
        Err(PsdSaveRefusal::NoLayers)
    );

    let mut many = LayerTree::new();
    for i in 0..32_768usize {
        ok(many.add_pixel_layer_at(format!("L{i}"), rect(0, 0, 1, 1), None, i));
    }
    assert_eq!(
        psd_save_preflight(&many, (4, 4), opts),
        Err(PsdSaveRefusal::TooManyLayers { count: 32_768 })
    );

    // `write_psd` refuses through the same checks before writing a byte.
    let mut doc = Doc::new((4, 4));
    ok(doc.tree.add_adjustment_layer_at(
        "C",
        Adjustment::Curves(aurora_core::CurvesParams::default()),
        None,
        0,
    ));
    let mut out = Cursor::new(Vec::new());
    let mut source = &doc.composite;
    let result = write_psd(
        &mut out,
        &doc.tree,
        (4, 4),
        &mut source,
        &mut doc.store,
        opts,
    );
    assert!(matches!(
        result,
        Err(IoError::PsdSaveRefused(
            PsdSaveRefusal::CurvesNotYetSupported { .. }
        ))
    ));
    assert!(out.into_inner().is_empty());
}

#[test]
fn lr16_block_is_padded_to_4_outside_its_length() {
    let mut unpadded = 0;
    for name in ["A", "AB", "ABC", "ABCD", "ABCDE"] {
        let mut doc = Doc::new((5, 3));
        let px = image(5, 3, |x, y| [level(x), level(y), level(x + y), 1.0]);
        doc.layer(name, rect(0, 0, 5, 3), Some((&px, 0, 0)));
        let (bytes, _) = ok(doc.write());
        let at = section_at(&bytes);
        let section = be32(&bytes, at) as usize;
        let lr16 = be32(&bytes, at + 4 + 4 + 4 + 8) as usize;
        assert_eq!(section % 4, 0, "{name}");
        // Section: layer info length, global mask length, `8BIM` `Lr16`
        // and its length, then the block and its padding.
        let pad = section - 20 - lr16;
        assert!(pad < 4, "{name}: {pad}");
        assert_eq!(pad, (4 - lr16 % 4) % 4, "{name}");
        if !lr16.is_multiple_of(4) {
            unpadded += 1;
        }
    }
    assert!(unpadded > 0, "at least one case needs padding");
}

#[test]
fn big_endian_fields_are_big_endian() {
    let mut doc = Doc::new((0x0102, 0x0003));
    let px = image(2, 1, |_, _| [1.0, 0.0, 0.0, 1.0]);
    let id = doc.layer("A", rect(-0x0203, 1, 300, 2), Some((&px, 0, 0)));
    ok(doc.tree.set_opacity(id, 0.5));
    let (bytes, _) = ok(doc.write());
    assert_eq!(bytes.get(..4), Some(&b"8BPS"[..]));
    assert_eq!(bytes.get(4..6), Some(&[0, 1][..]));
    assert_eq!(bytes.get(12..14), Some(&[0, 4][..]));
    assert_eq!(bytes.get(14..18), Some(&[0, 0, 0, 3][..]), "height");
    assert_eq!(bytes.get(18..22), Some(&[0, 0, 1, 2][..]), "width");
    assert_eq!(bytes.get(22..24), Some(&[0, 16][..]));
    assert_eq!(bytes.get(24..26), Some(&[0, 3][..]));
    assert_eq!(
        bytes.get(first_record(&bytes) - 2..first_record(&bytes)),
        Some(&[0xFF, 0xFF][..])
    );
    let record = parse_record(&bytes, first_record(&bytes));
    // One tile occupied: the extent is that tile's part of the bounds.
    assert_eq!(record.rect, [1, -0x0203, 3, -0x0203 + 256]);
    assert_eq!(
        bytes.get(first_record(&bytes) + 4..first_record(&bytes) + 8),
        Some(&[0xFF, 0xFF, 0xFD, 0xFD][..])
    );
    assert_eq!(
        byte(&bytes, first_record(&bytes) + 18 + 24 + 8),
        128,
        "opacity"
    );
    // Every channel id is a big-endian i16: -1 is FF FF.
    assert_eq!(record.channels.first().map(|c| c.0), Some(-1));
    assert_eq!(
        bytes.get(first_record(&bytes) + 18..first_record(&bytes) + 20),
        Some(&[0xFF, 0xFF][..])
    );
}

#[test]
fn dissolve_is_itemised() {
    let line = "Dissolve layers will look different in apps other than Photoshop.";
    let mut tree = LayerTree::new();
    let a = ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    let b = ok(tree.add_pixel_layer("B", rect(0, 0, 4, 4), None));
    let opts = PsdSaveOptions::default();
    assert_eq!(
        psd_save_preflight(&tree, (4, 4), opts),
        Ok(super::write::PsdWriteReport::default())
    );
    ok(tree.set_blend_mode(a, BlendMode::Dissolve));
    ok(tree.set_blend_mode(b, BlendMode::Dissolve));
    ok(tree.set_opacity(b, 0.3333));
    let report = ok(psd_save_preflight(&tree, (4, 4), opts));
    assert_eq!(
        report.items,
        [
            "1 layer opacity or fill value will be rounded to Photoshop's 255 steps.",
            line
        ]
    );
}

#[test]
fn streaming_writer_is_correct_across_several_tiles_per_side() {
    // 3 x 3 tiles and a partial fourth: 700 x 600, with the centre tile
    // and one corner tile absent (never painted), offset at (-37, 11).
    let (w, h) = (700u32, 600u32);
    let mut doc = Doc::new((640, 620));
    let id = ok(doc.tree.add_pixel_layer("Big", rect(-37, 11, w, h), None));
    let Some(surface) = doc.tree.surface_id(id) else {
        unreachable!()
    };
    let value = |x: u32, y: u32| [level(x), level(y), level(x ^ y), level(x / 7 + y)];
    for ty in 0..3u32 {
        for tx in 0..3u32 {
            if (tx, ty) == (1, 1) || (tx, ty) == (2, 0) {
                continue;
            }
            let (x0, y0) = (tx * 256, ty * 256);
            let tw = (w - x0).min(256);
            let th = (h - y0).min(256);
            let img = image(tw, th, |x, y| value(x0 + x, y0 + y));
            ok(crate::write_into_store_at(
                &img,
                &mut doc.store,
                surface,
                x0,
                y0,
            ));
        }
    }
    let (bytes, outcome) = ok(doc.write());
    // The extent is the occupied tiles' union: the whole bounds here.
    let record = parse_record(&bytes, first_record(&bytes));
    assert_eq!(record.rect, [11, -37, 11 + 600, -37 + 700]);
    let back = read_back(&bytes);
    let Some(p) = back.pixels.first() else {
        unreachable!()
    };
    assert_eq!((p.image.width(), p.image.height()), (w, h));
    for y in 0..h {
        for x in 0..w {
            let (tx, ty) = (x / 256, y / 256);
            let want = if (tx, ty) == (1, 1) || (tx, ty) == (2, 0) {
                [0.0; 4]
            } else {
                value(x, y)
            };
            let i = ((y * w + x) * 4) as usize;
            for (c, want) in want.iter().enumerate() {
                let got = p.image.samples().get(i + c).map(|v| v.to_f32());
                assert_eq!(got, Some(*want), "({x}, {y}) channel {c}");
            }
        }
    }
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("big.psd");
    ok(std::fs::write(&path, &bytes));
    ok(verify_psd_file(&path, &outcome.expected));
}

#[test]
fn extent_is_the_occupied_tiles_not_the_bounds() {
    let mut doc = Doc::new((64, 64));
    let id = ok(doc
        .tree
        .add_pixel_layer("Sparse", rect(0, 0, 1000, 900), None));
    let Some(surface) = doc.tree.surface_id(id) else {
        unreachable!()
    };
    let px = image(3, 2, |_, _| [1.0, 0.5, 0.25, 1.0]);
    ok(crate::write_into_store_at(
        &px,
        &mut doc.store,
        surface,
        600,
        300,
    ));
    let (bytes, _) = ok(doc.write());
    let record = parse_record(&bytes, first_record(&bytes));
    assert_eq!(record.rect, [256, 512, 512, 768]);
}

#[test]
fn verify_catches_a_changed_pixel_and_a_changed_name() {
    let mut doc = Doc::new((8, 8));
    let px = image(8, 8, |x, y| [level(x), level(y), 0.5, 1.0]);
    doc.layer("A", rect(0, 0, 8, 8), Some((&px, 0, 0)));
    let (bytes, outcome) = ok(doc.write());
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("v.psd");
    ok(std::fs::write(&path, &bytes));
    ok(verify_psd_file(&path, &outcome.expected));
    let mut bad = outcome.expected.clone();
    if let Some(l) = bad.layers.first_mut() {
        l.sample_hash ^= 1;
    }
    assert!(matches!(
        verify_psd_file(&path, &bad),
        Err(IoError::PsdVerifyFailed { .. })
    ));
    let mut bad: PsdExpectation = outcome.expected.clone();
    if let Some(l) = bad.layers.first_mut() {
        l.name = "B".to_owned();
    }
    assert!(verify_psd_file(&path, &bad).is_err());
    // Only the reader pass sees the decoded `f16` samples.
    let mut bad = outcome.expected.clone();
    if let Some(l) = bad.layers.first_mut() {
        l.decoded_hash ^= 1;
    }
    assert!(verify_psd_file(&path, &bad).is_err());
    let mut bad = outcome.expected;
    bad.merged_hash ^= 1;
    assert!(verify_psd_file(&path, &bad).is_err());
}

// ---------------------------------------------------------------------
// replace_verified
// ---------------------------------------------------------------------

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = ok(std::fs::read_dir(dir))
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn flat_doc() -> Doc {
    let mut doc = Doc::new((16, 16));
    let px = image(16, 16, |x, y| [level(x), level(y), 0.5, 1.0]);
    doc.layer("A", rect(0, 0, 16, 16), Some((&px, 0, 0)));
    doc
}

fn save(doc: &mut Doc, target: &Path, short: bool) -> Result<PsdWriteOutcome, IoError> {
    let Doc {
        tree,
        store,
        canvas,
        composite,
        ..
    } = doc;
    replace_verified(
        target,
        |file| {
            let mut source: &Image = composite;
            let outcome = write_psd(
                &mut *file,
                tree,
                *canvas,
                &mut source,
                store,
                PsdSaveOptions::default(),
            )?;
            if short {
                let len = file.metadata()?.len();
                file.set_len(len / 2)?;
            }
            Ok(outcome)
        },
        |path, outcome| verify_psd_file(path, &outcome.expected),
    )
}

#[test]
fn replace_verified_writes_and_verifies() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"old"));
    let mut doc = flat_doc();
    let outcome = ok(save(&mut doc, &target, false));
    ok(verify_psd_file(&target, &outcome.expected));
    assert_eq!(dir_listing(dir.path()), ["out.psd"]);
}

#[test]
fn replace_verified_leaves_target_untouched_on_write_error() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"the user's file"));
    let result: Result<(), IoError> = replace_verified(
        &target,
        |file| {
            std::io::Write::write_all(file, b"partial")?;
            Err(IoError::Io(std::io::Error::other("disk full")))
        },
        |_, ()| Ok(()),
    );
    assert!(result.is_err());
    assert_eq!(ok(std::fs::read(&target)), b"the user's file");
    assert_eq!(dir_listing(dir.path()), ["out.psd"]);
}

#[test]
fn replace_verified_leaves_target_untouched_on_verify_error() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"the user's file"));
    let mut verified = None;
    let result = replace_verified(
        &target,
        |file| {
            std::io::Write::write_all(file, b"new bytes")?;
            Ok(7)
        },
        |path, value| {
            verified = Some((ok(std::fs::read(path)), *value));
            Err(IoError::PsdVerifyFailed {
                what: "test".to_owned(),
            })
        },
    );
    assert!(matches!(result, Err(IoError::PsdVerifyFailed { .. })));
    // `verify` saw the synced temp file's bytes, by path.
    assert_eq!(verified, Some((b"new bytes".to_vec(), 7)));
    assert_eq!(ok(std::fs::read(&target)), b"the user's file");
    assert_eq!(dir_listing(dir.path()), ["out.psd"]);
}

#[test]
fn replace_verified_leaves_target_untouched_on_short_write() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"the user's file"));
    let mut doc = flat_doc();
    let result = save(&mut doc, &target, true);
    assert!(result.is_err(), "a truncated file must not verify");
    assert_eq!(ok(std::fs::read(&target)), b"the user's file");
    assert_eq!(dir_listing(dir.path()), ["out.psd"]);
}

#[test]
fn replace_verified_never_reuses_a_user_tmp_name() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    let user = ["out.psd.tmp", ".out.psd.tmp", ".out.psd..tmp", "out.tmp"];
    for name in user {
        ok(std::fs::write(dir.path().join(name), name.as_bytes()));
    }
    let mut seen = Vec::new();
    let mut doc = flat_doc();
    let Doc {
        tree,
        store,
        canvas,
        composite,
        ..
    } = &mut doc;
    ok(replace_verified(
        &target,
        |file| {
            let mut source: &Image = composite;
            write_psd(
                &mut *file,
                tree,
                *canvas,
                &mut source,
                store,
                PsdSaveOptions::default(),
            )
        },
        |path, outcome| {
            seen.push(path.to_path_buf());
            verify_psd_file(path, &outcome.expected)
        },
    ));
    let Some(temp) = seen.first() else {
        unreachable!("verified once")
    };
    let temp_name = temp.file_name().map(|n| n.to_string_lossy().into_owned());
    assert!(!user.iter().any(|u| Some((*u).to_owned()) == temp_name));
    assert_eq!(temp.parent(), Some(dir.path()), "beside the target");
    for name in user {
        assert_eq!(ok(std::fs::read(dir.path().join(name))), name.as_bytes());
    }
    let mut want: Vec<String> = user.iter().map(|s| (*s).to_owned()).collect();
    want.push("out.psd".to_owned());
    want.sort();
    assert_eq!(dir_listing(dir.path()), want);
}

#[cfg(unix)]
#[test]
fn replace_verified_keeps_an_existing_targets_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"old"));
    ok(std::fs::set_permissions(
        &target,
        std::fs::Permissions::from_mode(0o640),
    ));
    let mut doc = flat_doc();
    ok(save(&mut doc, &target, false));
    let mode = ok(std::fs::metadata(&target)).permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

// ---------------------------------------------------------------------
// Fixtures for the independent reader (scripts/verify_psd_write.py)
// ---------------------------------------------------------------------

/// Writes `fixture.psd` and `fixture.json` (what it should contain) into
/// `$AURORA_PSD_FIXTURE_DIR`, for `scripts/verify_psd_write.py` to check
/// with psd-tools. Ignored: run it by name.
#[test]
#[ignore = "writes fixtures for scripts/verify_psd_write.py"]
#[allow(clippy::too_many_lines)]
fn write_psd_fixtures_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = Path::new(&out);
    let mut doc = Doc::new((300, 270));
    doc.composite = image(300, 270, |x, y| {
        [level(x), level(y), level(x + y), level(x / 9 + 1)]
    });
    let a = image(300, 270, |x, y| [level(x), level(y / 3), 0.5, 1.0]);
    let b = image(40, 30, |x, y| [level(y), 1.0, level(x), level(x * y + 1)]);
    let c = image(20, 10, |x, _| [0.0, 0.25, level(x), 1.0]);
    doc.layer("Background", rect(0, 0, 300, 270), Some((&a, 0, 0)));
    let top = doc.layer("Ébauche 草稿", rect(250, 240, 40, 30), Some((&b, 0, 0)));
    ok(doc.tree.set_opacity(top, 128.0 / 255.0));
    ok(doc.tree.set_blend_mode(top, BlendMode::Multiply));
    let hidden = doc.layer("Hidden", rect(5, 6, 20, 10), Some((&c, 0, 0)));
    ok(doc.tree.set_visible(hidden, false));
    ok(doc.tree.set_blend_mode(hidden, BlendMode::Screen));
    ok(doc.tree.set_fill_opacity(hidden, 51.0 / 255.0));
    let (bytes, outcome) = ok(doc.write());
    ok(std::fs::write(out.join("fixture.psd"), &bytes));

    let q = |v: f16| quantize_u16(v.to_f32());
    let plane_json = |img: &Image, c: usize| -> String {
        let rows: Vec<String> = img
            .samples()
            .chunks_exact(4 * img.width() as usize)
            .map(|row| {
                let v: Vec<String> = row
                    .chunks_exact(4)
                    .map(|p| q(p.get(c).copied().unwrap_or(f16::ZERO)).to_string())
                    .collect();
                format!("[{}]", v.join(","))
            })
            .collect();
        format!("[{}]", rows.join(","))
    };
    let layer_json = |name: &str,
                      key: &str,
                      opacity: u8,
                      fill: u8,
                      visible: bool,
                      l: i64,
                      t: i64,
                      img: &Image| {
        format!(
            "{{\"name\":{name:?},\"blend\":{key:?},\"opacity\":{opacity},\"fill\":{fill},\
             \"visible\":{visible},\"left\":{l},\"top\":{t},\"width\":{},\"height\":{},\
             \"r\":{},\"g\":{},\"b\":{},\"a\":{}}}",
            img.width(),
            img.height(),
            plane_json(img, 0),
            plane_json(img, 1),
            plane_json(img, 2),
            plane_json(img, 3)
        )
    };
    // The merged planes exactly as the writer mattes them: in `f32`.
    let merged_json = |c: usize| -> String {
        let rows: Vec<String> = doc
            .composite
            .samples()
            .chunks_exact(4 * 300)
            .map(|row| {
                let v: Vec<String> = row
                    .chunks_exact(4)
                    .map(|p| {
                        let s = |c: usize| p.get(c).map_or(0.0, |v| v.to_f32());
                        let a = s(3);
                        let v = if c == 3 { a } else { s(c) * a + 1.0 - a };
                        quantize_u16(v).to_string()
                    })
                    .collect();
                format!("[{}]", v.join(","))
            })
            .collect();
        format!("[{}]", rows.join(","))
    };
    let json = format!(
        "{{\"width\":300,\"height\":270,\"layers\":[{},{},{}],\"merged\":{{\"r\":{},\"g\":{},\"b\":{},\"a\":{}}}}}",
        layer_json("Background", "norm", 255, 255, true, 0, 0, &a),
        layer_json("Ébauche 草稿", "mul ", 128, 255, true, 250, 240, &b),
        layer_json("Hidden", "scrn", 255, 51, false, 5, 6, &c),
        merged_json(0),
        merged_json(1),
        merged_json(2),
        merged_json(3)
    );
    ok(std::fs::write(out.join("fixture.json"), json));
    assert_eq!(outcome.expected.layers.len(), 3);

    // 0.198.0: `locks.psd` — one layer per lock combination and a locked
    // group — for `scripts/verify_psd_write.py`'s psd-tools check.
    let mut locked = Doc::new((16, 16));
    let mut spec = Vec::new();
    for bits in 0..8u8 {
        let lock = aurora_doc::LayerLock {
            transparency: bits & 1 != 0,
            pixels: bits & 2 != 0,
            position: bits & 4 != 0,
        };
        let name = format!("L{bits}");
        let id = locked.layer(&name, rect(0, 0, 16, 16), None);
        ok(locked.tree.set_lock(id, lock));
        spec.push(format!(
            "{{\"name\":{name:?},\"transparency\":{},\"pixels\":{},\"position\":{}}}",
            lock.transparency, lock.pixels, lock.position
        ));
    }
    let (locked_bytes, _) = ok(locked.write());
    ok(std::fs::write(out.join("locks.psd"), locked_bytes));
    ok(std::fs::write(
        out.join("locks.json"),
        format!("[{}]", spec.join(",")),
    ));

    // 0.201.0: `clips.psd` — a base, two clipped layers, a free layer and a
    // clipped layer on it — for psd-tools' `clipping_layer` check.
    let (clips_bytes, names) = clipping_document_bytes();
    ok(std::fs::write(out.join("clips.psd"), clips_bytes));
    ok(std::fs::write(
        out.join("clips.json"),
        format!(
            "[{}]",
            names
                .iter()
                .map(|(name, clipped)| format!("{{\"name\":{name:?},\"clipping\":{clipped}}}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ));
}

/// Bottom to top: base, c1 (clipped), c2 (clipped), free, c3 (clipped).
fn clipping_document_bytes() -> (Vec<u8>, Vec<(String, bool)>) {
    let mut doc = Doc::new((16, 16));
    let mut names = Vec::new();
    for (name, clipped) in [
        ("base", false),
        ("c1", true),
        ("c2", true),
        ("free", false),
        ("c3", true),
    ] {
        let id = doc.layer(name, rect(0, 0, 16, 16), None);
        if clipped {
            ok(doc.tree.set_clipped(id, true));
        }
        names.push((name.to_owned(), clipped));
    }
    let (bytes, _) = ok(doc.write());
    (bytes, names)
}

/// 0.201.0: clipping round-trips through the writer, the writer's own
/// verifier and the reader, and a record's clipping byte is exactly the
/// layer's.
#[test]
fn clipping_round_trips_through_write_verify_and_read() {
    let (bytes, names) = clipping_document_bytes();
    let back = read_back(&bytes);
    for (name, clipped) in names {
        let found = back
            .layers
            .roots()
            .iter()
            .copied()
            .find(|&id| back.layers.name(id) == Some(name.as_str()));
        let Some(id) = found else {
            unreachable!("layer {name}");
        };
        assert_eq!(back.layers.clip_base(id).is_some(), clipped, "{name}");
    }
    // c2's base is the layer two below it, c3's is "free".
    let by = |n: &str| {
        back.layers
            .roots()
            .iter()
            .copied()
            .find(|&id| back.layers.name(id) == Some(n))
    };
    assert_eq!(by("c2").and_then(|c| back.layers.clip_base(c)), by("base"));
    assert_eq!(by("c3").and_then(|c| back.layers.clip_base(c)), by("free"));
    assert!(
        !back
            .report
            .items
            .iter()
            .any(|i| i.contains("clipped layer"))
    );
}

#[allow(dead_code)]
fn assert_source_object_safe(_: &mut dyn PsdCompositeSource) {}

// ---------------------------------------------------------------------
// PackBits, the 2 GiB limit, and the composite source (0.176.0 review)
// ---------------------------------------------------------------------

fn packbits_round_trip(row: &[u8]) -> Vec<u8> {
    let packed = super::write::pack_bits(row);
    let mut out = Vec::new();
    ok(unpack_bits(&packed, &mut out, row.len()));
    assert_eq!(out, row, "PackBits round trip");
    assert!(
        packed.len() <= row.len() + row.len().div_ceil(128),
        "{} bytes packed into {}",
        row.len(),
        packed.len()
    );
    packed
}

#[test]
fn pack_bits_edge_rows() {
    assert!(packbits_round_trip(&[]).is_empty());
    assert_eq!(packbits_round_trip(&[7]), [0, 7]);
    assert_eq!(packbits_round_trip(&[7; 128]), [129, 7]);
    assert_eq!(packbits_round_trip(&[7; 129]), [129, 7, 0, 7]);
    assert_eq!(packbits_round_trip(&[7; 130]), [129, 7, 255, 7]);
    let literal: Vec<u8> = (0..128u8).collect();
    let packed = packbits_round_trip(&literal);
    assert_eq!(packed.first(), Some(&127));
    assert_eq!(packed.len(), 129);
    // A two-byte run that starts a packet is a repeat.
    assert_eq!(packbits_round_trip(&[5, 5]), [255, 5]);
}

#[test]
fn pack_bits_stays_within_n_plus_n_over_128() {
    let mut patterns: Vec<Vec<u8>> = vec![
        // `x y y` repeated: the pattern that grew by a third when a
        // literal broke at every two-byte run.
        [1u8, 2, 2].repeat(2000),
        [1u8, 2].repeat(3000), // all alternating
        [1u8, 1, 2, 3, 3, 4].repeat(1000),
        [9u8, 9, 9, 1, 2, 2].repeat(1000),
    ];
    let mut seed = 0x1234_5678_u64;
    for len in [1usize, 2, 3, 127, 128, 129, 255, 256, 1000, 6000] {
        let mut row = Vec::with_capacity(len);
        for _ in 0..len {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // Few symbols, so short runs of every length occur.
            row.push((seed >> 61) as u8);
        }
        patterns.push(row);
    }
    for row in &patterns {
        packbits_round_trip(row);
    }
    // `x y y` must cost no more than an all-literal encoding.
    let xyy = [1u8, 2, 2].repeat(2000);
    let packed = super::write::pack_bits(&xyy);
    assert!(
        packed.len() <= 6000 + 6000usize.div_ceil(128),
        "{}",
        packed.len()
    );
}

#[test]
fn a_30000_px_worst_case_row_fits_a_u16_row_count() {
    // 16-bit samples alternating in a way that defeats every repeat: the
    // packed row must still fit the PSD row table's `u16`.
    for pattern in [[1u8, 2, 2].as_slice(), &[1u8, 2], &[0u8, 1, 2, 3]] {
        let row: Vec<u8> = pattern.iter().copied().cycle().take(60_000).collect();
        let packed = packbits_round_trip(&row);
        assert!(u16::try_from(packed.len()).is_ok(), "{}", packed.len());
    }
}

#[test]
fn a_psd_reaching_the_size_limit_is_refused_with_a_psb_hint() {
    let mut doc = Doc::new((64, 64));
    let px = image(64, 64, |x, y| {
        [level(x * 3 + y), level(y * 7 + x), level(x ^ y), 1.0]
    });
    doc.layer("A", rect(0, 0, 64, 64), Some((&px, 0, 0)));
    let mut out = Cursor::new(Vec::new());
    let mut source = &doc.composite;
    let result = super::write::write_psd_limited(
        &mut out,
        &doc.tree,
        doc.canvas,
        &mut source,
        &mut doc.store,
        PsdSaveOptions::default(),
        super::write::WriteLimits {
            max_bytes: 5_000,
            ..Default::default()
        },
    );
    let Err(err) = result else {
        unreachable!("over the limit")
    };
    // It stopped within about one channel band (plus `BufWriter`'s 8 KiB
    // flushed on drop) of the limit, not after writing everything.
    let written = out.get_ref().len();
    let full = ok(doc.write()).0.len();
    assert!(full > 30_000, "{full}");
    assert!(written < 20_000, "{written} of {full}");
    assert!(matches!(err, IoError::PsdWriteTooLarge { .. }), "{err:?}");
    assert!(err.to_string().contains("use .psb"), "{err}");
    assert_eq!(super::write::PSD_MAX_FILE_BYTES, (1 << 31) - 1);
}

#[test]
fn image_composite_source_reports_a_bad_row() {
    let img = image(3, 2, |_, _| [0.0; 4]);
    let mut source = &img;
    let mut short = vec![f16::ZERO; 8];
    assert!(matches!(
        source.read_row(0, &mut short),
        Err(IoError::PsdCompositeRow { y: 0 })
    ));
    let mut row = vec![f16::ZERO; 12];
    assert!(matches!(
        source.read_row(2, &mut row),
        Err(IoError::PsdCompositeRow { y: 2 })
    ));
    assert!(source.read_row(1, &mut row).is_ok());
}

#[cfg(unix)]
#[test]
fn replace_verified_keeps_0o664_exactly_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"old"));
    ok(std::fs::set_permissions(
        &target,
        std::fs::Permissions::from_mode(0o664),
    ));
    let mut doc = flat_doc();
    ok(save(&mut doc, &target, false));
    let mode = ok(std::fs::metadata(&target)).permissions().mode() & 0o777;
    assert_eq!(mode, 0o664);
}

// ---------------------------------------------------------------------
// 0.177.0: ICC, round-trip budget, verify skip, early size refusal,
// symbolic links
// ---------------------------------------------------------------------

#[test]
fn icc_resource_is_srgb_and_reader_still_tags_srgb() {
    let mut doc = flat_doc();
    let (bytes, outcome) = ok(doc.write());
    // Resource 1039 is the first and only resource, holding a full ICC
    // profile that parses and is byte-for-byte the built-in sRGB one.
    let len = be32(&bytes, 30) as usize;
    let res = bytes_at(&bytes, 34, len);
    assert_eq!(res.get(..4), Some(&b"8BIM"[..]));
    assert_eq!(be16(res, 4), 1039);
    let size = be32(res, 8) as usize;
    let profile = bytes_at(res, 12, size);
    // lcms2's built-in sRGB, apart from the creation date and time its
    // serialiser stamps into header bytes 24..36.
    let srgb = ok(IccProfile::srgb().to_bytes());
    assert_eq!(profile.len(), srgb.len());
    let unstamped = |p: &[u8]| -> Vec<u8> {
        p.iter()
            .enumerate()
            .filter(|(i, _)| !(24..36).contains(i))
            .map(|(_, b)| *b)
            .collect()
    };
    assert_eq!(unstamped(profile), unstamped(&srgb));
    assert_eq!(be32(profile, 0) as usize, size, "ICC header size field");
    assert_eq!(profile.get(36..40), Some(&b"acsp"[..]), "ICC signature");
    assert_eq!(profile.get(16..20), Some(&b"RGB "[..]), "colour space");
    ok(IccProfile::from_bytes(profile));
    // The reader opens it as sRGB with nothing reported.
    let back = read_back(&bytes);
    assert!(back.report.is_empty(), "{:?}", back.report);
    // (The reader tags every layer with `IccProfile::srgb()` and reports
    // a profile that does not name sRGB; the empty report is that check.)
    assert!(!back.pixels.is_empty());
    assert!(outcome.report.is_empty());
}

#[test]
fn a_file_aurora_cannot_reopen_is_itemised() {
    let mut doc = flat_doc();
    let mut out = Cursor::new(Vec::new());
    let mut source = &doc.composite;
    let outcome = ok(super::write::write_psd_limited(
        &mut out,
        &doc.tree,
        doc.canvas,
        &mut source,
        &mut doc.store,
        PsdSaveOptions::default(),
        super::write::WriteLimits {
            reopen_budget: 16 * 16 - 1,
            ..Default::default()
        },
    ));
    assert_eq!(
        outcome.report.items,
        [
            "Aurora can't reopen this file yet: its layers add up to more than 268,435,456 \
          pixels. Photoshop can open it."
        ]
    );
    // At the real budget the same document reports nothing.
    assert!(ok(doc.write()).1.report.is_empty());
}

#[test]
fn verify_skips_the_reader_pass_past_its_budget() {
    let mut doc = flat_doc();
    let (bytes, outcome) = ok(doc.write());
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("v.psd");
    ok(std::fs::write(&path, &bytes));
    let mut bad = outcome.expected.clone();
    if let Some(l) = bad.layers.first_mut() {
        l.decoded_hash ^= 1;
    }
    // Within the budget the reader pass runs and sees the difference...
    assert!(super::write::verify_psd_file_with_budget(&path, &bad, 16 * 16).is_err());
    // ...past it only the raw pass runs, which does not look at it.
    ok(super::write::verify_psd_file_with_budget(
        &path,
        &bad,
        16 * 16 - 1,
    ));
    // The raw pass still catches a raw difference past the budget.
    let mut raw_bad = outcome.expected;
    if let Some(l) = raw_bad.layers.first_mut() {
        l.sample_hash ^= 1;
    }
    assert!(super::write::verify_psd_file_with_budget(&path, &raw_bad, 0).is_err());
}

#[test]
fn a_psd_that_cannot_fit_is_refused_before_a_byte_is_written() {
    let mut doc = Doc::new((64, 64));
    let px = image(64, 64, |x, y| [level(x), level(y), 0.5, 1.0]);
    doc.layer("A", rect(0, 0, 64, 64), Some((&px, 0, 0)));
    let mut out = Cursor::new(Vec::new());
    let mut source = &doc.composite;
    let result = super::write::write_psd_limited(
        &mut out,
        &doc.tree,
        doc.canvas,
        &mut source,
        &mut doc.store,
        PsdSaveOptions::default(),
        super::write::WriteLimits {
            // Below even PackBits' best case for these rows.
            max_bytes: 2_000,
            ..Default::default()
        },
    );
    let Err(err) = result else {
        unreachable!("refused")
    };
    assert!(err.to_string().contains("use .psb"), "{err}");
    assert!(out.into_inner().is_empty(), "nothing written");
}

#[cfg(unix)]
#[test]
fn saving_through_a_symlink_replaces_the_real_file_and_keeps_the_link() {
    let dir = ok(tempfile::tempdir());
    let real_dir = dir.path().join("real");
    ok(std::fs::create_dir(&real_dir));
    let real = real_dir.join("art.psd");
    ok(std::fs::write(&real, b"old"));
    let link = dir.path().join("link.psd");
    ok(std::os::unix::fs::symlink(&real, &link));
    let mut doc = flat_doc();
    let outcome = ok(save(&mut doc, &link, false));
    assert!(
        ok(std::fs::symlink_metadata(&link))
            .file_type()
            .is_symlink()
    );
    ok(verify_psd_file(&real, &outcome.expected));
    assert_eq!(
        dir_listing(&real_dir),
        ["art.psd"],
        "temp beside the real file, gone"
    );
    // A dangling link is refused, and nothing is created.
    let dangling = dir.path().join("dangling.psd");
    ok(std::os::unix::fs::symlink(
        dir.path().join("missing.psd"),
        &dangling,
    ));
    let mut doc = flat_doc();
    assert!(save(&mut doc, &dangling, false).is_err());
    assert!(!dir.path().join("missing.psd").exists());
}

#[test]
fn write_verified_temp_touches_nothing_until_persist() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("out.psd");
    ok(std::fs::write(&target, b"the user's file"));
    let (value, temp) = ok(crate::write_verified_temp(
        &target,
        |file| {
            std::io::Write::write_all(file, b"new")?;
            Ok(3)
        },
        |_, _| Ok(()),
    ));
    assert_eq!(value, 3);
    assert_eq!(ok(std::fs::read(&target)), b"the user's file");
    let temp_path = temp.path().to_path_buf();
    assert!(temp_path.exists());
    drop(temp);
    assert!(!temp_path.exists(), "dropped = removed");
    assert_eq!(ok(std::fs::read(&target)), b"the user's file");
    let ((), temp) = ok(crate::write_verified_temp(
        &target,
        |file| std::io::Write::write_all(file, b"new").map_err(IoError::from),
        |_, ()| Ok(()),
    ));
    ok(temp.persist());
    assert_eq!(ok(std::fs::read(&target)), b"new");
    assert_eq!(dir_listing(dir.path()), ["out.psd"]);
}

// ---------------------------------------------------------------------
// Groups and masks (0.178.0, round R3)
// ---------------------------------------------------------------------

impl Doc {
    /// Adds a layer on top of `parent`'s children (or the roots).
    fn layer_in(
        &mut self,
        name: &str,
        bounds: Rect,
        pixels: Option<&Image>,
        parent: Option<LayerId>,
    ) -> LayerId {
        let id = ok(self.tree.add_pixel_layer(name, bounds, parent));
        if let Some(img) = pixels {
            let Some(surface) = self.tree.surface_id(id) else {
                unreachable!("pixel layer");
            };
            ok(crate::write_into_store_at(
                img,
                &mut self.store,
                surface,
                0,
                0,
            ));
        }
        id
    }

    fn group(&mut self, name: &str, parent: Option<LayerId>) -> LayerId {
        ok(self.tree.add_group(name, parent))
    }

    /// Adds a mask at `bounds`, its coverage `f(x, y)` (mask-local)
    /// painted over the `paint` part of it (mask-local x, y, w, h).
    fn mask(
        &mut self,
        id: LayerId,
        bounds: Rect,
        paint: (u32, u32, u32, u32),
        f: impl Fn(u32, u32) -> f32,
    ) {
        ok(self.tree.add_mask(id, bounds));
        let Some(surface) = self.tree.mask_surface_id(id) else {
            unreachable!("mask surface");
        };
        let (x, y, w, h) = paint;
        ok(aurora_doc::write_mask_coverage_region(
            &mut self.store,
            surface,
            (x, y),
            w,
            h,
            |cx, cy| f(x + cx, y + cy),
        ));
    }
}

/// The tree's shape, top-to-bottom: `name` or `name(children)`.
fn shape(tree: &LayerTree, ids: &[LayerId]) -> String {
    ids.iter()
        .map(|&id| {
            let name = tree.name(id).unwrap_or_default().to_owned();
            match tree.children(id) {
                Some(children) if tree.kind(id).is_some_and(aurora_doc::LayerKind::is_group) => {
                    format!("{name}({})", shape(tree, children))
                }
                _ => name,
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A re-read document with its masks written into a store of its own.
struct Reopened {
    doc: super::PsdDocument,
    _dir: tempfile::TempDir,
    store: TileStore,
}

fn reopen(bytes: &[u8]) -> Reopened {
    let doc = read_back(bytes);
    let (dir, mut store) = store();
    for mask in &doc.masks {
        ok(super::write_mask_pixels(mask, &doc.layers, &mut store));
    }
    Reopened {
        doc,
        _dir: dir,
        store,
    }
}

/// Aurora's effective mask coverage at a document pixel — the app's
/// `apply_mask`: inside the bounds the painted coverage (unpainted is
/// `1.0`), outside them `0.0`; then `inverted`; then the density.
fn coverage_at(tree: &LayerTree, store: &mut TileStore, id: LayerId, x: i64, y: i64) -> f32 {
    let density = tree.mask(id).map_or(1.0, |m| m.density);
    density * shown_at(tree, store, id, x, y) + (1.0 - density)
}

/// [`coverage_at`] before the density: the coverage after `inverted`.
fn shown_at(tree: &LayerTree, store: &mut TileStore, id: LayerId, x: i64, y: i64) -> f32 {
    let Some(mask) = tree.mask(id) else {
        return 1.0;
    };
    let b = mask.bounds;
    let raw = if b.contains_point(x, y) {
        let (Ok(lx), Ok(ly)) = (u32::try_from(x - b.x), u32::try_from(y - b.y)) else {
            unreachable!("inside the bounds")
        };
        let tile = aurora_tile::TileId {
            x: lx / aurora_tile::TILE,
            y: ly / aurora_tile::TILE,
        };
        match tree.mask_surface_id(id) {
            Some(surface) if store.contains_tile(surface, tile) => {
                let t = ok(store.get(surface, tile));
                let at = (((ly % aurora_tile::TILE) * aurora_tile::TILE + lx % aurora_tile::TILE)
                    * 4) as usize;
                t.texels()
                    .get(at..at + 4)
                    .map_or(1.0, aurora_doc::read_mask_coverage)
            }
            _ => 1.0,
        }
    } else {
        0.0
    };
    if mask.inverted { 1.0 - raw } else { raw }
}

/// Every record's (`lsct` data, rect, channel ids, flags, mask data) in
/// file order, parsed straight from the bytes.
type Rec = (Option<Vec<u8>>, [i32; 4], Vec<i16>, Vec<u8>, String);

/// Record `i` of `recs`.
fn rec(recs: &[Rec], i: usize) -> &Rec {
    recs.get(i).unwrap_or_else(|| unreachable!("record {i}"))
}

fn records(bytes: &[u8]) -> Vec<Rec> {
    let count = usize::from(
        be16(bytes, first_record(bytes) - 2)
            .cast_signed()
            .unsigned_abs(),
    );
    let mut at = first_record(bytes);
    let mut out = Vec::new();
    for _ in 0..count {
        let r = parse_record(bytes, at);
        let mask_len = be32(bytes, r.extra_at) as usize;
        let mask = bytes_at(bytes, r.extra_at + 4, mask_len).to_vec();
        let ranges_at = r.extra_at + 4 + mask_len;
        let name_at = ranges_at + 4 + be32(bytes, ranges_at) as usize;
        let plen = usize::from(byte(bytes, name_at));
        let name = String::from_utf8_lossy(bytes_at(bytes, name_at + 1, plen)).into_owned();
        let mut block = name_at + (plen + 1).next_multiple_of(4);
        let mut section = None;
        while block < r.end {
            let key = bytes_at(bytes, block + 4, 4).to_vec();
            let len = be32(bytes, block + 8) as usize;
            if key == b"lsct" {
                section = Some(bytes_at(bytes, block + 12, len).to_vec());
            }
            block += 12 + len;
        }
        let ids = r.channels.iter().map(|(id, _)| *id).collect();
        out.push((section, r.rect, ids, mask, name));
        let _ = r.flags;
        at = r.end;
    }
    out
}

fn blend_key_of(bytes: &[u8], index: usize) -> [u8; 4] {
    let mut at = first_record(bytes);
    for _ in 0..index {
        at = parse_record(bytes, at).end;
    }
    let r = parse_record(bytes, at);
    let blend_at = at + 18 + 6 * r.channels.len();
    let mut key = [0u8; 4];
    key.copy_from_slice(bytes_at(bytes, blend_at + 4, 4));
    key
}

fn px(w: u32, h: u32, seed: u32) -> Image {
    image(w, h, |x, y| {
        [level(x + seed), level(y), level(x + y + seed), 1.0]
    })
}

/// Three levels of groups with siblings above and below at every level.
fn nested_doc() -> (Doc, [LayerId; 3]) {
    let mut doc = Doc::new((12, 10));
    // Added bottom first at each level.
    doc.layer_in("Bottom", rect(0, 0, 12, 10), Some(&px(12, 10, 0)), None);
    let g1 = doc.group("G1", None);
    doc.layer_in("L1b", rect(1, 1, 4, 4), Some(&px(4, 4, 1)), Some(g1));
    let g2 = doc.group("G2", Some(g1));
    doc.layer_in("L2b", rect(2, 2, 3, 3), Some(&px(3, 3, 2)), Some(g2));
    let g3 = doc.group("G3", Some(g2));
    doc.layer_in("L3", rect(3, 3, 5, 2), Some(&px(5, 2, 3)), Some(g3));
    doc.layer_in("L2a", rect(4, 1, 2, 6), Some(&px(2, 6, 4)), Some(g2));
    doc.layer_in("L1a", rect(0, 5, 6, 5), Some(&px(6, 5, 5)), Some(g1));
    doc.layer_in("Top", rect(6, 0, 6, 10), Some(&px(6, 10, 6)), None);
    (doc, [g1, g2, g3])
}

#[test]
fn nested_groups_round_trip_order_and_membership() {
    let (mut doc, _) = nested_doc();
    let want = shape(&doc.tree, doc.tree.roots());
    assert_eq!(want, "Top,G1(L1a,G2(L2a,G3(L3),L2b),L1b),Bottom");
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    // Two records per group.
    assert_eq!(outcome.expected.layers.len(), 7 + 2 * 3);
    let back = read_back(&bytes);
    assert!(back.report.items.is_empty(), "{:?}", back.report);
    assert_eq!(shape(&back.layers, back.layers.roots()), want);
    // Pixels: every layer's decoded image equals what was written.
    assert_eq!(back.pixels.len(), 7);
    // And the verifier (both passes) accepts the file.
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("nested.psd"), false));
}

#[test]
fn group_divider_precedes_children_folder_follows() {
    let mut doc = Doc::new((4, 4));
    let g = doc.group("Group", None);
    doc.layer_in("Child", rect(0, 0, 4, 4), Some(&px(4, 4, 0)), Some(g));
    let (bytes, outcome) = ok(doc.write());
    let recs = records(&bytes);
    assert_eq!(recs.len(), 3);
    let kinds: Vec<_> = outcome.expected.layers.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        vec![
            super::write::PsdRecordKind::GroupEnd,
            super::write::PsdRecordKind::Layer,
            super::write::PsdRecordKind::Group
        ]
    );
    // Bottom: the bounding divider, type 3, empty, four 2-byte channels.
    let (section, rect0, ids, mask, name) = rec(&recs, 0);
    assert_eq!(section.as_deref(), Some(3u32.to_be_bytes().as_slice()));
    assert_eq!(*rect0, [0; 4]);
    assert_eq!(ids, &vec![-1, 0, 1, 2]);
    assert!(mask.is_empty());
    assert_eq!(name, super::write::PSD_GROUP_END_NAME);
    let divider = parse_record(&bytes, first_record(&bytes));
    assert!(divider.channels.iter().all(|&(_, len)| len == 2));
    // Middle: the child. Top: the open folder, `8BIM` + its blend key.
    assert_eq!(rec(&recs, 1).0, None);
    assert_eq!(rec(&recs, 1).4, "Child");
    let mut folder = 1u32.to_be_bytes().to_vec();
    folder.extend_from_slice(b"8BIMnorm");
    assert_eq!(rec(&recs, 2).0.as_deref(), Some(folder.as_slice()));
    assert_eq!(rec(&recs, 2).4, "Group");
    let back = read_back(&bytes);
    assert_eq!(shape(&back.layers, back.layers.roots()), "Group(Child)");
}

#[test]
fn group_blend_and_props_round_trip() {
    let mut doc = Doc::new((4, 4));
    doc.layer_in("Base", rect(0, 0, 4, 4), Some(&px(4, 4, 0)), None);
    let g = doc.group("Mul", None);
    doc.layer_in("In", rect(0, 0, 4, 4), Some(&px(4, 4, 1)), Some(g));
    ok(doc.tree.set_blend_mode(g, BlendMode::Multiply));
    ok(doc.tree.set_opacity(g, 128.0 / 255.0));
    ok(doc.tree.set_fill_opacity(g, 51.0 / 255.0));
    ok(doc.tree.set_visible(g, false));
    let normal = doc.group("Plain", None);
    doc.layer_in("In2", rect(0, 0, 2, 2), Some(&px(2, 2, 2)), Some(normal));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    // Records: Base, end, In, Mul, end, In2, Plain.
    assert_eq!(blend_key_of(&bytes, 3), *b"mul ");
    assert_eq!(blend_key_of(&bytes, 6), *b"norm");
    let recs = records(&bytes);
    assert_eq!(
        bytes_at(rec(&recs, 3).0.as_deref().unwrap_or_default(), 4, 8),
        b"8BIMmul "
    );
    assert_eq!(
        bytes_at(rec(&recs, 6).0.as_deref().unwrap_or_default(), 4, 8),
        b"8BIMnorm"
    );
    for (section, ..) in &recs {
        assert!(
            !section
                .as_deref()
                .is_some_and(|s| s.windows(4).any(|w| w == b"pass")),
            "a Normal/Multiply group is never written as Pass Through"
        );
    }
    let back = read_back(&bytes);
    let l = &back.layers;
    let mul = l
        .roots()
        .iter()
        .copied()
        .find(|&id| l.name(id) == Some("Mul"));
    let Some(mul) = mul else {
        unreachable!("group read back")
    };
    assert_eq!(l.blend_mode(mul), Some(BlendMode::Multiply));
    assert_eq!(l.opacity(mul), Some(128.0 / 255.0));
    assert_eq!(l.fill_opacity(mul), Some(51.0 / 255.0));
    assert_eq!(l.visible(mul), Some(false));
    assert!(
        back.report.items.is_empty(),
        "no Pass Through note: {:?}",
        back.report
    );
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("props.psd"), false));
}

/// A layer with a mask at (2, 3) 5x4, painted over its left 3 columns.
fn masked_doc() -> (Doc, LayerId) {
    let mut doc = Doc::new((10, 9));
    let id = doc.layer_in("M", rect(0, 0, 10, 9), Some(&px(10, 9, 0)), None);
    doc.mask(id, rect(2, 3, 5, 4), (0, 0, 3, 4), |x, y| level(x + 2 * y));
    (doc, id)
}

/// The effective coverage agrees everywhere the layer has pixels (the
/// canvas, here) — and, for a mask that is not inverted, around it too.
/// An inverted mask is read back with the layer's region as its bounds
/// (shown outside the PSD rectangle), so past the region, where the layer
/// has no pixels to show, it reads hidden.
fn assert_same_coverage(doc: &mut Doc, id: LayerId, back: &mut Reopened) {
    let back_id = back.doc.layers.roots().first().copied();
    let Some(back_id) = back_id else {
        unreachable!("a layer")
    };
    let inverted = doc.tree.mask(id).is_some_and(|m| m.inverted);
    let margin = if inverted { 0 } else { 2 };
    let (w, h) = (i64::from(doc.canvas.0), i64::from(doc.canvas.1));
    for y in -margin..h + margin {
        for x in -margin..w + margin {
            let want = coverage_at(&doc.tree, &mut doc.store, id, x, y);
            let got = coverage_at(&back.doc.layers, &mut back.store, back_id, x, y);
            assert_eq!(got.to_bits(), want.to_bits(), "coverage at ({x}, {y})");
        }
    }
}

#[test]
fn mask_bounds_and_default_round_trip() {
    let (mut doc, id) = masked_doc();
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    let recs = records(&bytes);
    let (_, _, ids, mask, _) = rec(&recs, 0);
    assert_eq!(ids, &vec![-1, 0, 1, 2, -2]);
    assert_eq!(mask.len(), 20);
    // top, left, bottom, right in document coordinates; default 0; flags 0.
    let edge = |i: usize| be32(mask, 4 * i).cast_signed();
    assert_eq!([edge(0), edge(1), edge(2), edge(3)], [3, 2, 7, 7]);
    assert_eq!(bytes_at(mask, 16, 4), &[0, 0, 0, 0]);
    let mut back = reopen(&bytes);
    let back_id = back
        .doc
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    let m = back.doc.layers.mask(back_id).cloned();
    let Some(m) = m else {
        unreachable!("mask read back")
    };
    assert_eq!(m.bounds, rect(2, 3, 5, 4));
    assert!(m.enabled && !m.inverted);
    assert_eq!(m.density.to_bits(), 1.0f32.to_bits());
    assert_same_coverage(&mut doc, id, &mut back);
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("mask.psd"), false));
}

#[test]
fn mask_outside_bounds_round_trips() {
    // Aurora's coverage outside a mask's bounds is 0.0 (hidden), so the
    // default colour is 0; inverted, it is 1.0 and the default 255.
    for inverted in [false, true] {
        let (mut doc, id) = masked_doc();
        ok(doc.tree.set_mask_inverted(id, inverted));
        assert_eq!(
            coverage_at(&doc.tree, &mut doc.store, id, 0, 0).to_bits(),
            f32::from(u8::from(inverted)).to_bits()
        );
        let (bytes, _) = ok(doc.write());
        let recs = records(&bytes);
        assert_eq!(byte(&rec(&recs, 0).3, 16), if inverted { 255 } else { 0 });
        let mut back = reopen(&bytes);
        assert_same_coverage(&mut doc, id, &mut back);
        let dir = ok(tempfile::tempdir());
        ok(save(&mut doc, &dir.path().join("outside.psd"), false));
    }
}

#[test]
fn inverted_mask_baked_and_flag_clear() {
    let (mut doc, id) = masked_doc();
    ok(doc.tree.set_mask_inverted(id, true));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty());
    let recs = records(&bytes);
    let flags = byte(&rec(&recs, 0).3, 17);
    assert_eq!(flags & 0x04, 0, "invert flag clear");
    assert_eq!(byte(&rec(&recs, 0).3, 16), 255, "default colour flipped");
    let mut back = reopen(&bytes);
    let back_id = back
        .doc
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    let m = back.doc.layers.mask(back_id).cloned();
    assert!(m.is_some_and(|m| !m.inverted), "baked, not flagged");
    // Painted 0.25 at mask (1, 0) reads back as 0.75.
    assert_eq!(
        coverage_at(&back.doc.layers, &mut back.store, back_id, 3, 3).to_bits(),
        (1.0 - level(1)).to_bits()
    );
    assert_same_coverage(&mut doc, id, &mut back);
}

#[test]
fn mask_density_parameter_round_trips() {
    let (mut doc, id) = masked_doc();
    ok(doc.tree.set_mask_density(id, 128.0 / 255.0));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    let recs = records(&bytes);
    let mask = rec(&recs, 0).3.clone();
    assert_eq!(byte(&mask, 17) & 0x10, 0x10, "parameters follow");
    assert_eq!(bytes_at(&mask, 18, 2), &[0x01, 128], "user density only");
    let mut back = reopen(&bytes);
    let back_id = back
        .doc
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    assert_eq!(
        back.doc.layers.mask(back_id).map(|m| m.density),
        Some(128.0 / 255.0)
    );
    assert!(back.doc.report.items.is_empty(), "{:?}", back.doc.report);
    assert_same_coverage(&mut doc, id, &mut back);
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("density.psd"), false));
}

#[test]
fn mask_density_rounding_itemised() {
    let (mut doc, id) = masked_doc();
    ok(doc.tree.set_mask_density(id, 0.3));
    let report = ok(psd_save_preflight(
        &doc.tree,
        doc.canvas,
        PsdSaveOptions::default(),
    ));
    assert_eq!(
        report.items,
        vec!["1 layer mask density will be rounded to Photoshop's 255 steps.".to_owned()]
    );
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(outcome.report, report);
    let recs = records(&bytes);
    assert_eq!(byte(&rec(&recs, 0).3, 19), 77);
    let back = read_back(&bytes);
    let back_id = back
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    assert_eq!(
        back.layers.mask(back_id).map(|m| m.density),
        Some(77.0 / 255.0)
    );
    // A density that rounds to full drops the block, and is still itemised.
    ok(doc.tree.set_mask_density(id, 0.999));
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(outcome.report.items.len(), 1);
    assert_eq!(bytes_at(&rec(&records(&bytes), 0).3, 17, 3), &[0, 0, 0]);
}

#[test]
fn disabled_mask_round_trips() {
    let (mut doc, id) = masked_doc();
    ok(doc.tree.set_mask_enabled(id, false));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty());
    assert_eq!(byte(&rec(&records(&bytes), 0).3, 17), 0x02);
    let mut back = reopen(&bytes);
    let back_id = back
        .doc
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    let m = back.doc.layers.mask(back_id).cloned();
    assert!(m.is_some_and(|m| !m.enabled), "still disabled");
    // Its coverage survives, for when it is enabled again.
    ok(doc.tree.set_mask_enabled(id, true));
    ok(back.doc.layers.set_mask_enabled(back_id, true));
    assert_same_coverage(&mut doc, id, &mut back);
    ok(doc.tree.set_mask_enabled(id, false));
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("disabled.psd"), false));
}

#[test]
fn group_mask_round_trips() {
    let mut doc = Doc::new((8, 8));
    let g = doc.group("Masked group", None);
    doc.layer_in("In", rect(0, 0, 8, 8), Some(&px(8, 8, 0)), Some(g));
    doc.mask(g, rect(1, 2, 6, 5), (1, 1, 4, 3), |x, y| level(x * y + 1));
    ok(doc.tree.set_mask_density(g, 204.0 / 255.0));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    let recs = records(&bytes);
    // The folder (top record) carries the mask; the divider does not.
    assert!(rec(&recs, 0).3.is_empty());
    assert_eq!(rec(&recs, 2).2, vec![-1, 0, 1, 2, -2]);
    assert_eq!(rec(&recs, 2).3.len(), 20);
    let mut back = reopen(&bytes);
    let bg = back
        .doc
        .layers
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    let m = back.doc.layers.mask(bg).cloned();
    let Some(m) = m else {
        unreachable!("group mask read back")
    };
    assert_eq!(m.bounds, rect(1, 2, 6, 5));
    assert_eq!(m.density.to_bits(), (204.0f32 / 255.0).to_bits());
    for y in -1..10i64 {
        for x in -1..10i64 {
            assert_eq!(
                coverage_at(&back.doc.layers, &mut back.store, bg, x, y).to_bits(),
                coverage_at(&doc.tree, &mut doc.store, g, x, y).to_bits(),
                "({x}, {y})"
            );
        }
    }
    let dir = ok(tempfile::tempdir());
    ok(save(&mut doc, &dir.path().join("group-mask.psd"), false));
}

#[test]
fn record_count_limit_counts_dividers() {
    let opts = PsdSaveOptions::default();
    let mut tree = LayerTree::new();
    let g = ok(tree.add_group("G", None));
    ok(tree.add_pixel_layer("In", rect(0, 0, 1, 1), Some(g)));
    // 1 group (2 records) + 1 child + 32,764 roots = 32,767: fits.
    for i in 0..32_764usize {
        ok(tree.add_pixel_layer_at(format!("L{i}"), rect(0, 0, 1, 1), None, i));
    }
    assert!(psd_save_preflight(&tree, (4, 4), opts).is_ok());
    // One more layer is 32,768 records though only 32,767 layers exist.
    ok(tree.add_pixel_layer_at("One more", rect(0, 0, 1, 1), None, 0));
    let refusal = psd_save_preflight(&tree, (4, 4), opts);
    assert_eq!(
        refusal,
        Err(PsdSaveRefusal::TooManyLayers { count: 32_768 })
    );
    let Err(refusal) = refusal else {
        unreachable!()
    };
    assert!(
        refusal.to_string().contains("a group takes two"),
        "{refusal}"
    );
}

#[test]
fn group_depth_limit_respected() {
    let max = super::MAX_GROUP_DEPTH;
    let mut doc = Doc::new((2, 2));
    let mut parent = None;
    for depth in 0..max {
        parent = Some(doc.group(&format!("G{depth}"), parent));
    }
    doc.layer_in("Deepest", rect(0, 0, 2, 2), Some(&px(2, 2, 0)), parent);
    let (bytes, outcome) = ok(doc.write());
    assert_eq!(outcome.expected.layers.len(), 2 * max + 1);
    let back = read_back(&bytes);
    assert_eq!(
        shape(&back.layers, back.layers.roots()),
        shape(&doc.tree, doc.tree.roots())
    );
    // One level deeper is refused before a byte is written — the reader
    // would refuse the file.
    let innermost = parent;
    let deeper = ok(doc.tree.add_group("Too deep", innermost));
    let _ = deeper;
    assert_eq!(
        psd_save_preflight(&doc.tree, (2, 2), PsdSaveOptions::default()),
        Err(PsdSaveRefusal::GroupsTooDeep {
            depth: max + 1,
            max
        })
    );
    let result = doc.write();
    assert!(matches!(
        result,
        Err(IoError::PsdSaveRefused(
            PsdSaveRefusal::GroupsTooDeep { .. }
        ))
    ));
}

/// One record's JSON for `scripts/verify_psd_write.py`'s group pass:
/// the node's props, its mask (bbox, default colour, disabled, density
/// and every written `u16` sample) and, for a pixel layer, its planes;
/// a group's children bottom-to-top.
#[allow(clippy::cast_sign_loss, clippy::single_match_else)] // values in 0..=1
fn node_json(doc: &mut Doc, id: LayerId) -> String {
    let t = &doc.tree;
    let name = t.name(id).unwrap_or_default().to_owned();
    let key = psd_blend_key(t.blend_mode(id).unwrap_or_default());
    let key = String::from_utf8_lossy(&key).into_owned();
    let byte = |v: f32| (v * 255.0).round() as u32;
    let props = format!(
        "\"name\":{name:?},\"blend\":{key:?},\"opacity\":{},\"fill\":{},\"visible\":{}",
        byte(t.opacity(id).unwrap_or(1.0)),
        byte(t.fill_opacity(id).unwrap_or(1.0)),
        t.visible(id).unwrap_or(true)
    );
    let mask = match t.mask(id).cloned() {
        None => "null".to_owned(),
        Some(m) => {
            let b = m.bounds;
            let mut rows = Vec::new();
            for y in 0..i64::from(b.height) {
                let mut row = Vec::new();
                for x in 0..i64::from(b.width) {
                    // The written sample: coverage (inverted baked), no density.
                    let c = shown_at(&doc.tree, &mut doc.store, id, b.x + x, b.y + y);
                    row.push(quantize_u16(c).to_string());
                }
                rows.push(format!("[{}]", row.join(",")));
            }
            let density = if m.density < 1.0 {
                ((m.density * 255.0).round() as u32).to_string()
            } else {
                "null".to_owned()
            };
            format!(
                "{{\"bbox\":[{},{},{},{}],\"default\":{},\"disabled\":{},\"density\":{density},\
                 \"data\":[{}]}}",
                b.x,
                b.y,
                b.right(),
                b.bottom(),
                if m.inverted { 255 } else { 0 },
                !m.enabled,
                rows.join(",")
            )
        }
    };
    match doc.tree.kind(id).cloned() {
        Some(aurora_doc::LayerKind::Group { children }) => {
            let kids: Vec<String> = children.iter().rev().map(|&c| node_json(doc, c)).collect();
            format!(
                "{{\"kind\":\"group\",{props},\"mask\":{mask},\"children\":[{}]}}",
                kids.join(",")
            )
        }
        _ => {
            let Some(surface) = doc.tree.surface_id(id) else {
                unreachable!("pixel layer")
            };
            let Some(aurora_doc::LayerKind::Pixel { bounds }) = doc.tree.kind(id).cloned() else {
                unreachable!("pixel layer")
            };
            let img = ok(crate::read_from_store(
                &mut doc.store,
                surface,
                bounds.width,
                bounds.height,
            ));
            let plane = |c: usize| -> String {
                let rows: Vec<String> = img
                    .samples()
                    .chunks_exact(4 * img.width() as usize)
                    .map(|row| {
                        let v: Vec<String> = row
                            .chunks_exact(4)
                            .map(|p| quantize_u16(p.get(c).map_or(0.0, |v| v.to_f32())).to_string())
                            .collect();
                        format!("[{}]", v.join(","))
                    })
                    .collect();
                format!("[{}]", rows.join(","))
            };
            format!(
                "{{\"kind\":\"pixel\",{props},\"mask\":{mask},\"left\":{},\"top\":{},\
                 \"width\":{},\"height\":{},\"r\":{},\"g\":{},\"b\":{},\"a\":{}}}",
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                plane(0),
                plane(1),
                plane(2),
                plane(3)
            )
        }
    }
}

#[test]
#[ignore = "writes fixtures for scripts/verify_psd_write.py"]
fn write_psd_group_fixtures_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = Path::new(&out);
    let mut doc = Doc::new((40, 30));
    doc.layer_in("Background", rect(0, 0, 40, 30), Some(&px(40, 30, 0)), None);
    let outer = doc.group("Outer", None);
    ok(doc.tree.set_blend_mode(outer, BlendMode::Multiply));
    ok(doc.tree.set_opacity(outer, 200.0 / 255.0));
    let masked = doc.layer_in(
        "Masked",
        rect(4, 3, 20, 12),
        Some(&px(20, 12, 1)),
        Some(outer),
    );
    doc.mask(masked, rect(6, 5, 15, 9), (2, 1, 10, 6), |x, y| {
        level(x + y)
    });
    ok(doc.tree.set_mask_density(masked, 128.0 / 255.0));
    let inner = doc.group("Inner 内側", Some(outer));
    ok(doc.tree.set_visible(inner, false));
    // 0.179.0: a Pass Through group, so psd-tools reads `pass` back from
    // both its record and its `lsct`.
    ok(doc.tree.set_blend_mode(inner, BlendMode::PassThrough));
    ok(doc.tree.set_fill_opacity(inner, 51.0 / 255.0));
    doc.layer_in("Deep", rect(10, 10, 8, 8), Some(&px(8, 8, 2)), Some(inner));
    doc.mask(inner, rect(2, 2, 30, 20), (0, 0, 30, 20), |x, y| {
        level(x * y)
    });
    ok(doc.tree.set_mask_inverted(inner, true));
    ok(doc.tree.set_mask_enabled(inner, false));
    doc.layer_in(
        "Sibling",
        rect(0, 20, 12, 10),
        Some(&px(12, 10, 3)),
        Some(outer),
    );
    let top = doc.layer_in("Top", rect(30, 0, 10, 30), Some(&px(10, 30, 4)), None);
    ok(doc.tree.set_blend_mode(top, BlendMode::Screen));
    let (bytes, outcome) = ok(doc.write());
    assert!(outcome.report.is_empty(), "{:?}", outcome.report);
    ok(std::fs::write(out.join("groups.psd"), &bytes));
    let roots: Vec<LayerId> = doc.tree.roots().iter().rev().copied().collect();
    let nodes: Vec<String> = roots.iter().map(|&id| node_json(&mut doc, id)).collect();
    let json = format!(
        "{{\"width\":40,\"height\":30,\"tree\":[{}]}}",
        nodes.join(",")
    );
    ok(std::fs::write(out.join("groups.json"), json));
}

/// A mask tile that was never painted (no tile at all in the store) is
/// coverage `1.0`, inside the bounds, and is written as such — the
/// common case of a mask added in the app and never painted, or painted
/// only in part of a wide mask (mutation-run finding, 0.178.0).
#[test]
fn unpainted_mask_tiles_write_full_coverage() {
    for painted in [false, true] {
        let mut doc = Doc::new((300, 10));
        doc.composite = px(300, 10, 0);
        let id = doc.layer_in("Wide", rect(0, 0, 300, 10), Some(&px(300, 10, 0)), None);
        if painted {
            // Only the first of the mask's two tile columns exists.
            doc.mask(id, rect(0, 0, 300, 10), (0, 0, 3, 3), |x, y| level(x + y));
        } else {
            ok(doc.tree.add_mask(id, rect(0, 0, 300, 10)));
        }
        let (bytes, outcome) = ok(doc.write());
        assert!(outcome.report.is_empty(), "{:?}", outcome.report);
        let mut back = reopen(&bytes);
        let back_id = back
            .doc
            .layers
            .roots()
            .first()
            .copied()
            .unwrap_or_else(|| unreachable!());
        assert_eq!(
            coverage_at(&back.doc.layers, &mut back.store, back_id, 280, 5).to_bits(),
            1.0f32.to_bits(),
            "painted: {painted}"
        );
        assert_same_coverage(&mut doc, id, &mut back);
        let dir = ok(tempfile::tempdir());
        ok(save(&mut doc, &dir.path().join("wide.psd"), false));
    }
}

// ---------------------------------------------------------------------
// Locks (0.198.0)
// ---------------------------------------------------------------------

/// Every lock combination, on a layer and on a group: the record flag bit 0
/// is transparency, an `lspf` block carries the rest (Lock All is exactly
/// `0x8000_0000`), the file verifies (raw and reader passes), reads back
/// equal, and the save reports nothing.
#[test]
fn every_lock_combination_round_trips_through_write_and_read_and_verifies() {
    use aurora_doc::LayerLock;
    for bits in 0..8u8 {
        let lock = LayerLock {
            transparency: bits & 1 != 0,
            pixels: bits & 2 != 0,
            position: bits & 4 != 0,
        };
        let mut doc = flat_doc();
        let group = ok(doc.tree.add_group("G", None));
        let child = ok(doc
            .tree
            .add_pixel_layer("C", rect(0, 0, 16, 16), Some(group)));
        let _ = child;
        let roots = doc.tree.roots().to_vec();
        for id in roots.iter().copied().chain([group]) {
            ok(doc.tree.set_lock(id, lock));
        }
        let dir = ok(tempfile::tempdir());
        let target = dir.path().join("locks.psd");
        let outcome = ok(save(&mut doc, &target, false));
        assert!(outcome.report.is_empty(), "{:?}", outcome.report);
        let bytes = ok(std::fs::read(&target));
        let back = read_back(&bytes);
        for id in back.layers.roots() {
            assert_eq!(back.layers.lock(*id), Some(lock), "{lock:?}");
        }
        let has_lspf = bytes.windows(4).any(|w| w == b"lspf");
        assert_eq!(has_lspf, lock.is_any());
        if lock == LayerLock::all() {
            assert!(
                bytes
                    .windows(8)
                    .any(|w| w == [b'l', b's', b'p', b'f', 0, 0, 0, 4])
                    && bytes.windows(4).any(|w| w == 0x8000_0000u32.to_be_bytes()),
                "Lock All is written as 0x80000000"
            );
        }
    }
}

#[test]
fn a_transparency_lock_sets_record_flag_bit_0_and_nothing_else() {
    let mut doc = Doc::new((4, 4));
    let id = doc.layer("L", rect(0, 0, 4, 4), None);
    ok(doc.tree.set_lock(
        id,
        aurora_doc::LayerLock {
            transparency: true,
            ..aurora_doc::LayerLock::none()
        },
    ));
    let (bytes, _) = ok(doc.write());
    let record = parse_record(&bytes, first_record(&bytes));
    assert_eq!(record.flags, 0x01);
}
