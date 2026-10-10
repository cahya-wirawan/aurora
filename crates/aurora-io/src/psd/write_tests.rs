//! Tests for the flat PSD writer (0.176.0, round R1).
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

/// Offset of the first layer record: header 26, colour data 4,
/// resources 4, section length 4, layer info length 4, global mask 4,
/// `8BIM` `Lr16` and its length 12, layer count 2.
const FIRST_RECORD: usize = 26 + 4 + 4 + 4 + 4 + 4 + 12 + 2;

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
    for mode in BlendMode::ALL {
        assert_eq!(blend_for_key(psd_blend_key(mode)), PsdBlend::Mode(mode));
    }
    let mut keys: Vec<[u8; 4]> = BlendMode::ALL.iter().map(|&m| psd_blend_key(m)).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 27);

    let mut doc = Doc::new((4, 4));
    let px = image(4, 4, |_, _| [0.5, 0.25, 0.75, 1.0]);
    for mode in BlendMode::ALL {
        let id = doc.layer(&format!("{mode:?}"), rect(0, 0, 4, 4), Some((&px, 0, 0)));
        ok(doc.tree.set_blend_mode(id, mode));
    }
    let (bytes, _) = ok(doc.write());
    let back = read_back(&bytes);
    let modes: Vec<_> = bottom_up(&back.layers)
        .iter()
        .map(|&id| back.layers.blend_mode(id))
        .collect();
    let want: Vec<_> = BlendMode::ALL.iter().map(|&m| Some(m)).collect();
    assert_eq!(modes, want);
}

#[test]
fn hidden_layer_sets_flag_bit_1() {
    for visible in [true, false] {
        let mut doc = Doc::new((4, 4));
        let id = doc.layer("L", rect(0, 0, 4, 4), None);
        ok(doc.tree.set_visible(id, visible));
        let (bytes, _) = ok(doc.write());
        let record = parse_record(&bytes, FIRST_RECORD);
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
    let record = parse_record(&bytes, FIRST_RECORD);
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
    let mut at = FIRST_RECORD;
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
    let section = be32(&bytes, 34) as usize;
    let merged = 38 + section;
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
        })
        .write();
    assert_eq!(bytes.len(), oracle.len());
    assert!(bytes == oracle, "byte-for-byte equal to the test writer");
}

#[test]
fn preflight_refuses_groups_masks_curves_and_oversize() {
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

    let mut groups = base();
    ok(groups.add_group("G", None));
    assert_eq!(
        psd_save_preflight(&groups, (4, 4), opts),
        Err(PsdSaveRefusal::GroupsNotYetSupported { count: 1 })
    );

    let mut masked = base();
    let id = masked
        .roots()
        .first()
        .copied()
        .unwrap_or_else(|| unreachable!());
    ok(masked.add_mask(id, rect(0, 0, 4, 4)));
    assert_eq!(
        psd_save_preflight(&masked, (4, 4), opts),
        Err(PsdSaveRefusal::MasksNotYetSupported { count: 1 })
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
    ok(doc.tree.add_group("G", None));
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
            PsdSaveRefusal::GroupsNotYetSupported { .. }
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
        let section = be32(&bytes, 34) as usize;
        let lr16 = be32(&bytes, 34 + 4 + 4 + 4 + 8) as usize;
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
        bytes.get(FIRST_RECORD - 2..FIRST_RECORD),
        Some(&[0xFF, 0xFF][..])
    );
    let record = parse_record(&bytes, FIRST_RECORD);
    // One tile occupied: the extent is that tile's part of the bounds.
    assert_eq!(record.rect, [1, -0x0203, 3, -0x0203 + 256]);
    assert_eq!(
        bytes.get(FIRST_RECORD + 4..FIRST_RECORD + 8),
        Some(&[0xFF, 0xFF, 0xFD, 0xFD][..])
    );
    assert_eq!(byte(&bytes, FIRST_RECORD + 18 + 24 + 8), 128, "opacity");
    // Every channel id is a big-endian i16: -1 is FF FF.
    assert_eq!(record.channels.first().map(|c| c.0), Some(-1));
    assert_eq!(
        bytes.get(FIRST_RECORD + 18..FIRST_RECORD + 20),
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
    let record = parse_record(&bytes, FIRST_RECORD);
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
    let record = parse_record(&bytes, FIRST_RECORD);
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
        1_000,
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
