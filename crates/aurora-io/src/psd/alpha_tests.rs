//! Tests for saved selections as PSD alpha channels (task 156).
#![allow(clippy::many_single_char_names)]
#![allow(clippy::indexing_slicing)] // fixed-size fixtures

use std::io::Cursor;
use std::sync::Arc;

use aurora_core::Rect;
use aurora_doc::{Selection, SelectionMask, SelectionShape};

use super::PsdDocument;
use super::alpha::{
    self, Caps, KIND_INVERTED, KIND_PLAIN, KIND_SPOT, ResourceChannel, read_channels_with,
};
use super::test_writer::TestPsd;
use super::write::PsdSaveOptions;
use super::write_tests::{Doc, ok, read_back};

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

/// The image resource section for `channels` (8BIM blocks).
fn resources(channels: &[ResourceChannel]) -> Vec<u8> {
    let mut out = Vec::new();
    for (id, body) in alpha::resource_blocks(channels) {
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
    }
    out
}

fn channel(name: &str, kind: u8) -> ResourceChannel {
    ResourceChannel {
        kind,
        ..ResourceChannel::alpha(name, 7)
    }
}

/// A flat RGB file `w x h` whose merged image has the colour planes white
/// and `alpha` planes after them.
fn flat(
    w: u32,
    h: u32,
    depth: u16,
    compression: u16,
    alpha: Vec<Vec<u8>>,
    res: Vec<u8>,
) -> Vec<u8> {
    let bps = usize::from(depth / 8).max(1);
    let colour = vec![0xFF; w as usize * h as usize * bps];
    let mut planes = vec![colour.clone(), colour.clone(), colour];
    let n = alpha.len();
    planes.extend(alpha);
    TestPsd::new(1, w, h, depth)
        .with(|f| {
            f.channels = 3 + n as u16;
            f.merged = Some(planes);
            f.merged_compression = compression;
            f.resources = res;
        })
        .write()
}

/// Coverage bytes of a selection over `w x h`.
fn coverage(selection: &Selection, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut row = vec![0u8; w as usize];
    for y in 0..h {
        selection.coverage_row_bytes(i64::from(y), 0, &mut row);
        out.extend_from_slice(&row);
    }
    out
}

fn open(bytes: &[u8]) -> PsdDocument {
    read_back(bytes)
}

/// A 6 x 4 ramp of distinct bytes, with a zero border so the box is trimmed.
fn ramp() -> Vec<u8> {
    let mut v = vec![0u8; 24];
    for y in 1..3usize {
        for x in 1..5usize {
            v[y * 6 + x] = (x * 40 + y * 3) as u8;
        }
    }
    v
}

fn one_alpha(depth: u16, compression: u16, plane: Vec<u8>, kind: u8) -> PsdDocument {
    open(&flat(
        6,
        4,
        depth,
        compression,
        vec![plane],
        resources(&[channel("Alpha 1", kind)]),
    ))
}

fn names(doc: &PsdDocument) -> Vec<&str> {
    doc.saved_selections
        .iter()
        .map(|(n, _)| n.as_str())
        .collect()
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

// ---------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------

#[test]
fn an_8_bit_alpha_channel_opens_as_a_saved_mask_in_every_compression() {
    for compression in 0..=3u16 {
        let doc = open(&flat(
            6,
            4,
            8,
            compression,
            vec![ramp()],
            resources(&[channel("Mine", KIND_PLAIN)]),
        ));
        assert_eq!(names(&doc), ["Mine"], "compression {compression}");
        let (_, selection) = &doc.saved_selections[0];
        assert_eq!(
            coverage(selection, 6, 4),
            ramp(),
            "compression {compression}"
        );
        assert!(matches!(selection.shape, SelectionShape::Mask(_)));
        // Trimmed to the box of the non-zero texels.
        assert_eq!(
            (
                selection.bounds.x,
                selection.bounds.y,
                selection.bounds.width,
                selection.bounds.height
            ),
            (1, 1, 4, 2)
        );
        assert!(
            doc.report
                .items
                .iter()
                .any(|i| i.contains("1 alpha channel opened as saved selection")),
            "{:?}",
            doc.report.items
        );
    }
}

#[test]
fn a_16_bit_alpha_channel_reads_exactly_for_a_written_byte_and_rounds_otherwise() {
    let bytes: Vec<u8> = ramp()
        .iter()
        .flat_map(|&b| (u16::from(b) * 257).to_be_bytes())
        .collect();
    let doc = one_alpha(16, 1, bytes, KIND_PLAIN);
    assert_eq!(coverage(&doc.saved_selections[0].1, 6, 4), ramp());
    // 0x8000 rounds to 128, 0x7fff to 127.
    let mut plane = vec![0u8; 6 * 4 * 2];
    plane[2..4].copy_from_slice(&0x8000u16.to_be_bytes());
    plane[4..6].copy_from_slice(&0x7fffu16.to_be_bytes());
    plane[6..8].copy_from_slice(&0xffffu16.to_be_bytes());
    let doc = one_alpha(16, 0, plane, KIND_PLAIN);
    let cov = coverage(&doc.saved_selections[0].1, 6, 4);
    assert_eq!(cov.get(1..4), Some(&[128u8, 127, 255][..]));
}

#[test]
fn a_32_bit_float_alpha_channel_clamps_and_reads_nan_as_zero() {
    let mut plane = vec![0u8; 6 * 4 * 4];
    for (i, v) in [0.5f32, 2.0, -1.0, f32::NAN, 1.0].iter().enumerate() {
        plane[(i + 1) * 4..(i + 2) * 4].copy_from_slice(&v.to_be_bytes());
    }
    let doc = one_alpha(32, 0, plane, KIND_PLAIN);
    let cov = coverage(&doc.saved_selections[0].1, 6, 4);
    assert_eq!(cov.get(..6), Some(&[0u8, 128, 255, 0, 0, 255][..]));
}

#[test]
fn kind_zero_is_read_inverted_and_spot_channels_are_left_out() {
    let doc = one_alpha(
        8,
        0,
        ramp().iter().map(|b| 255 - b).collect(),
        KIND_INVERTED,
    );
    // The inverted plane's zero border is 255 -> selected, so the inverse of
    // the ramp (255 - (255 - ramp)) is the ramp.
    assert_eq!(coverage(&doc.saved_selections[0].1, 6, 4), ramp());

    let res = resources(&[channel("Ink", KIND_SPOT), channel("Keep", KIND_PLAIN)]);
    let doc = open(&flat(6, 4, 8, 1, vec![vec![9; 24], ramp()], res));
    assert_eq!(names(&doc), ["Keep"]);
    assert!(
        doc.report
            .items
            .iter()
            .any(|i| i.contains("1 spot colour channel was not opened")),
        "{:?}",
        doc.report.items
    );
    assert!(
        !doc.report
            .items
            .iter()
            .any(|i| i.contains("alpha or spot colour"))
    );
}

#[test]
fn solid_white_is_a_rectangle_and_all_black_is_left_out() {
    let res = resources(&[channel("Black", KIND_PLAIN), channel("White", KIND_PLAIN)]);
    let doc = open(&flat(6, 4, 8, 0, vec![vec![0; 24], vec![255; 24]], res));
    assert_eq!(names(&doc), ["White"]);
    let selection = &doc.saved_selections[0].1;
    assert!(selection.is_rect());
    assert_eq!(
        selection.bounds,
        Rect {
            x: 0,
            y: 0,
            width: 6,
            height: 4
        }
    );
    assert!(
        doc.report
            .items
            .iter()
            .any(|i| i.contains("nothing selected"))
    );
}

#[test]
fn names_prefer_unicode_fall_back_and_are_made_unique() {
    let mut c = ResourceChannel::alpha("ascii", 3);
    c.name = "Ma\u{f1}ana".to_owned();
    let doc = open(&flat(6, 4, 8, 0, vec![ramp()], resources(&[c])));
    assert_eq!(names(&doc), ["Ma\u{f1}ana"]);
    // Only 1006 present: repeats get " 2", empty or control-character or
    // over-long names and missing ones get "Alpha N".
    let long = "x".repeat(70);
    let mut body = Vec::new();
    for n in [&b"Same"[..], b"Same", b"", b"a\x07b"] {
        body.push(n.len() as u8);
        body.extend_from_slice(n);
    }
    body.push(70);
    body.extend_from_slice(long.as_bytes());
    let mut res = Vec::new();
    res.extend_from_slice(b"8BIM\x03\xee\0\0");
    res.extend_from_slice(&(body.len() as u32).to_be_bytes());
    res.extend_from_slice(&body);
    if body.len() % 2 == 1 {
        res.push(0);
    }
    let doc = open(&flat(6, 4, 8, 0, vec![ramp(); 6], res));
    let got = names(&doc);
    assert_eq!(got.len(), 6);
    assert_eq!(&got[..2], ["Same", "Same 2"]);
    assert!(got[2].starts_with("Alpha"), "{got:?}");
    assert_eq!(got[3], "ab", "{got:?}");
    assert!(got[4].starts_with("Alpha"), "{got:?}");
    assert!(got[5].starts_with("Alpha"), "{got:?}");
    let unique: std::collections::BTreeSet<_> = got.iter().collect();
    assert_eq!(unique.len(), 6);
}

#[test]
fn damaged_resources_and_merged_images_never_fail_the_open() {
    for cut in [0usize, 3, 7, 11, 20] {
        let mut res = resources(&[channel("Alpha 1", KIND_PLAIN)]);
        res.truncate(res.len().saturating_sub(cut));
        let _ = open(&flat(6, 4, 8, 0, vec![ramp()], res));
    }
    // A header that promises a channel the merged image does not hold.
    let mut bytes = flat(
        6,
        4,
        8,
        0,
        vec![ramp()],
        resources(&[channel("A", KIND_PLAIN)]),
    );
    bytes.truncate(bytes.len() - 20);
    assert!(super::read(&bytes).map_or(true, |d| d.saved_selections.is_empty()));
    // A truncated RLE merged image.
    let mut bytes = flat(
        6,
        4,
        8,
        1,
        vec![ramp()],
        resources(&[channel("A", KIND_PLAIN)]),
    );
    let at = bytes.len();
    bytes.truncate(at - 10);
    assert!(super::read(&bytes).map_or(true, |d| d.saved_selections.is_empty()));
    // An unknown compression fails a flat file's own flattened-image read, as
    // before; it never yields selections.
    let mut bytes = flat(
        6,
        4,
        8,
        0,
        vec![ramp()],
        resources(&[channel("A", KIND_PLAIN)]),
    );
    let comp_at = bytes.len() - 6 * 4 * 4 - 2;
    bytes[comp_at..comp_at + 2].copy_from_slice(&9u16.to_be_bytes());
    assert!(super::read(&bytes).is_err());
    // A zip stream of the wrong size.
    let mut bytes = flat(
        6,
        4,
        8,
        2,
        vec![ramp()],
        resources(&[channel("A", KIND_PLAIN)]),
    );
    let at = bytes.len();
    bytes.truncate(at - 6);
    assert!(super::read(&bytes).map_or(true, |d| d.saved_selections.is_empty()));
}

#[test]
fn a_gray_and_a_cmyk_file_open_their_alpha_channels() {
    let res = resources(&[channel("G", KIND_PLAIN)]);
    let gray = TestPsd::new(1, 6, 4, 8)
        .with(|f| {
            f.color_mode = 1;
            f.channels = 2;
            f.merged = Some(vec![vec![0xFF; 24], ramp()]);
            f.resources = res.clone();
        })
        .write();
    assert_eq!(names(&open(&gray)), ["G"]);
    let cmyk = TestPsd::new(1, 6, 4, 8)
        .with(|f| {
            f.color_mode = 4;
            f.channels = 5;
            f.merged = Some(vec![
                vec![0; 24],
                vec![0; 24],
                vec![0; 24],
                vec![0; 24],
                ramp(),
            ]);
            f.resources = res;
        })
        .write();
    assert_eq!(names(&open(&cmyk)), ["G"]);
}

#[test]
fn the_caps_leave_channels_out_and_say_so() {
    let bytes = flat(6, 4, 8, 1, vec![ramp(), ramp(), ramp()], Vec::new());
    let outcome = |caps: Caps| {
        let mut src = ok(super::StreamSource::new(Cursor::new(bytes.clone())));
        // Header 26, colour data 4, resources 4, section 4 and its body.
        let window = super::Window {
            pos: 38 + u64::from(be32(&bytes, 34)),
            end: bytes.len() as u64,
        };
        let header = super::Header {
            version: 1,
            channels: 6,
            color_mode: 3,
            width: 6,
            height: 4,
            depth: 8,
        };
        let mut bodies = alpha::ResourceBodies::default();
        bodies.offer(1006, &[1, b'a', 1, b'b', 1, b'c']);
        ok(read_channels_with(
            &mut src, window, header, 3, 3, &bodies, caps,
        ))
    };
    let all = outcome(Caps::DEFAULT);
    assert_eq!(all.selections.len(), 3);
    let two = outcome(Caps {
        selections: 2,
        ..Caps::DEFAULT
    });
    assert_eq!((two.selections.len(), two.over_caps), (2, 1));
    // Each mask is 4 x 2 = 8 bytes.
    let by_bytes = outcome(Caps {
        bytes: 17,
        ..Caps::DEFAULT
    });
    assert_eq!((by_bytes.selections.len(), by_bytes.over_caps), (2, 1));
    // Exactly the cap is kept (two masks of 8 bytes in 16).
    let exact = outcome(Caps {
        bytes: 16,
        ..Caps::DEFAULT
    });
    assert_eq!((exact.selections.len(), exact.over_caps), (2, 1));
    let pixels = outcome(Caps {
        pixels: 23,
        ..Caps::DEFAULT
    });
    assert_eq!((pixels.selections.len(), pixels.over_caps), (0, 3));
}

// ---------------------------------------------------------------------
// Writing: planning and resources
// ---------------------------------------------------------------------

fn rect_sel(x: i64, y: i64, w: u32, h: u32) -> Selection {
    Selection::new(Rect {
        x,
        y,
        width: w,
        height: h,
    })
}

fn mask_sel(x: i64, y: i64, w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Selection {
    let mut data = Vec::new();
    for j in 0..h {
        for i in 0..w {
            data.push(f(i, j));
        }
    }
    let bounds = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    Selection::from_mask(Arc::new(ok(SelectionMask::new(bounds, data))))
}

#[test]
fn resource_blocks_have_photoshops_layout() {
    let blocks = alpha::resource_blocks(&[
        ResourceChannel::transparency(),
        ResourceChannel::alpha("Mask", 4),
    ]);
    let ids: Vec<u16> = blocks.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, [1006, 1045, 1053, 1077]);
    assert_eq!(blocks[0].1, b"\x0cTransparency\x04Mask");
    assert_eq!(blocks[2].1, [0, 0, 0, 0, 0, 0, 0, 4]);
    assert_eq!(&blocks[1].1[..4], 13u32.to_be_bytes());
    // Version 1, then 13 bytes a plane: RGB red, opacity, masked areas.
    let display = &blocks[3].1;
    assert_eq!(display.len(), 4 + 26);
    assert_eq!(&display[..4], 1u32.to_be_bytes());
    assert_eq!(&display[4..17], b"\0\0\xff\xff\0\0\0\0\0\0\0\x64\x01");
    assert_eq!(&display[17..30], b"\0\0\xff\xff\0\0\0\0\0\0\0\x32\x01");
    assert!(alpha::resource_blocks(&[]).is_empty());
}

#[test]
fn pascal_names_are_ascii_and_at_most_255_bytes() {
    assert_eq!(
        alpha::pascal_bytes("Sky \u{e9}\u{1f600}"),
        (b"Sky ??".to_vec(), false)
    );
    let (bytes, cut) = alpha::pascal_bytes(&"a".repeat(300));
    assert_eq!((bytes.len(), cut), (255, true));
    assert!(!alpha::pascal_bytes(&"a".repeat(255)).1);
}

#[test]
fn the_plan_itemises_what_it_leaves_out() {
    let inside = rect_sel(1, 1, 3, 3);
    let outside = rect_sel(50, 50, 3, 3);
    let past = rect_sel(-2, -2, 5, 5);
    let saved = [
        ("Inside", &inside),
        ("Outside", &outside),
        ("Past", &past),
        ("transparency", &inside),
    ];
    let plan = alpha::plan(&saved, (10, 10));
    let got: Vec<&str> = plan.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(got, ["Inside", "Past", "transparency 2"]);
    assert_eq!(plan.outside, ["Outside"]);
    assert_eq!(plan.clipped, ["Past"]);
    assert_eq!(
        plan.renamed,
        [("transparency".to_owned(), "transparency 2".to_owned())]
    );
    let ids: Vec<u32> = plan.channels.iter().map(|c| c.id).collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn the_plan_enforces_the_count_byte_and_size_caps() {
    let r = rect_sel(0, 0, 2, 2);
    let many: Vec<(String, &Selection)> = (0..70).map(|i| (format!("s{i:02}"), &r)).collect();
    let refs: Vec<(&str, &Selection)> = many.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let plan = alpha::plan(&refs, (4, 4));
    assert_eq!((plan.channels.len(), plan.over_caps.len()), (64, 6));
    // Three whole-canvas ellipses of 64 Mpx are more than 128 MiB of masks.
    let e = Selection::ellipse(Rect {
        x: 0,
        y: 0,
        width: 8192,
        height: 8192,
    });
    let saved = [("a", &e), ("b", &e), ("c", &e)];
    let plan = alpha::plan(&saved, (8192, 8192));
    assert_eq!((plan.channels.len(), plan.over_caps.len()), (2, 1));
    // A canvas past 64 Mpx writes none.
    let plan = alpha::plan(&saved, (8193, 8192));
    assert_eq!((plan.channels.len(), plan.canvas_too_large), (0, 3));
    // Inverted ones cost the whole canvas; a rectangle nothing.
    let mut inv = rect_sel(0, 0, 2, 2);
    inv.inverted = true;
    assert_eq!(alpha::read_back_bytes(&inv, (10, 10)), 100);
    assert_eq!(alpha::read_back_bytes(&r, (10, 10)), 0);
}

// ---------------------------------------------------------------------
// Writing: whole files
// ---------------------------------------------------------------------

fn write_with(doc: &mut Doc, saved: &[(&str, &Selection)]) -> (Vec<u8>, super::PsdWriteOutcome) {
    let mut out = Cursor::new(Vec::new());
    let mut source = &doc.composite;
    let outcome = ok(super::write_psd_with_selections(
        &mut out,
        &doc.tree,
        doc.canvas,
        &mut source,
        &mut doc.store,
        PsdSaveOptions::default(),
        saved,
    ));
    (out.into_inner(), outcome)
}

fn doc_with_a_layer(canvas: (u32, u32)) -> Doc {
    let mut doc = Doc::new(canvas);
    let img = super::write_tests::px(canvas.0, canvas.1, 3);
    doc.layer(
        "L",
        super::write_tests::rect(0, 0, canvas.0, canvas.1),
        Some((&img, 0, 0)),
    );
    doc
}

fn verify(bytes: &[u8], outcome: &super::PsdWriteOutcome) -> Result<(), crate::IoError> {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("a.psd");
    ok(std::fs::write(&path, bytes));
    super::verify_psd_file(&path, &outcome.expected)
}

#[test]
fn saved_selections_round_trip_bit_for_bit_and_pass_both_verify_passes() {
    for canvas in [(16u32, 12u32), (7, 5), (1, 1), (33, 3)] {
        let (w, h) = canvas;
        let rect = rect_sel(i64::from(w > 1), 0, w.saturating_sub(2).max(1), h);
        let ellipse = Selection::ellipse(Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        });
        let mut inverted = rect_sel(0, 0, w.div_ceil(2), h.div_ceil(2));
        inverted.inverted = true;
        let mask = mask_sel(0, 0, w, h, |x, y| {
            if (x + y) % 3 == 0 {
                0
            } else {
                ((x * 37 + y * 11) % 255 + 1) as u8
            }
        });
        let saved = [
            ("a rect", &rect),
            ("b ellipse", &ellipse),
            ("c inverted", &inverted),
            ("d mask", &mask),
        ];
        let mut doc = doc_with_a_layer(canvas);
        let (bytes, outcome) = write_with(&mut doc, &saved);
        ok(verify(&bytes, &outcome));
        assert_eq!(outcome.expected.alpha.len(), 4);
        // The header counts the alpha planes after the merged four.
        assert_eq!(u16::from_be_bytes([bytes[12], bytes[13]]), 8);
        let back = open(&bytes);
        for (name, sel) in &back.saved_selections {
            let Some(&(_, original)) = saved.iter().find(|s| s.0 == name) else {
                unreachable!("{name}")
            };
            assert_eq!(
                coverage(sel, w, h),
                coverage(original, w, h),
                "{name} on {canvas:?}"
            );
        }
        if w * h >= 12 {
            assert!(
                back.saved_selections
                    .iter()
                    .any(|(n, s)| n == "d mask" && !s.is_rect()),
                "{canvas:?}"
            );
        }
        // Writing what was read gives the same selections again.
        let mut doc2 = doc_with_a_layer(canvas);
        let again: Vec<(&str, &Selection)> = back
            .saved_selections
            .iter()
            .map(|(n, s)| (n.as_str(), s))
            .collect();
        let (bytes2, outcome2) = write_with(&mut doc2, &again);
        ok(verify(&bytes2, &outcome2));
        let back2 = open(&bytes2);
        assert_eq!(back.saved_selections, back2.saved_selections, "{canvas:?}");
        assert!(
            outcome
                .report
                .items
                .iter()
                .any(|i| i.contains("written as alpha channels")),
            "{:?}",
            outcome.report.items
        );
        // Task 157: unchecked in Photoshop, so it needs attention (asks).
        let line = outcome
            .report
            .entries
            .iter()
            .find(|e| e.text.contains("written as alpha channels"));
        assert_eq!(
            line.map(|e| e.category),
            Some(crate::ReportCategory::NeedsAttention)
        );
        assert!(line.is_some_and(|e| !e.affected.is_empty()));
    }
}

#[test]
fn every_byte_value_round_trips_through_the_16_bit_plane() {
    let mask = mask_sel(0, 0, 256, 1, |x, _| x as u8);
    let mut doc = doc_with_a_layer((256, 1));
    let (bytes, outcome) = write_with(&mut doc, &[("all", &mask)]);
    ok(verify(&bytes, &outcome));
    let back = open(&bytes);
    assert_eq!(
        coverage(&back.saved_selections[0].1, 256, 1),
        coverage(&mask, 256, 1)
    );
}

#[test]
fn a_file_without_saved_selections_is_unchanged() {
    let mut doc = doc_with_a_layer((9, 7));
    let (plain, _) = ok(doc.write());
    let (same, outcome) = write_with(&mut doc, &[]);
    assert_eq!(plain, same);
    assert!(outcome.expected.alpha.is_empty());
    assert_eq!(u16::from_be_bytes([plain[12], plain[13]]), 4);
    let resources_len = be32(&plain, 30) as usize;
    let resources = &plain[34..34 + resources_len];
    assert_eq!(&resources[..8], b"8BIM\x04\x0f\0\0", "only the ICC profile");
    // Selections that cannot be written leave the bytes alone too.
    let outside = rect_sel(100, 100, 2, 2);
    let (also, outcome) = write_with(&mut doc, &[("gone", &outside)]);
    assert_eq!(plain, also);
    assert!(
        outcome
            .report
            .items
            .iter()
            .any(|i| i.contains("outside the canvas"))
    );
}

#[test]
fn the_report_itemises_exceptions() {
    use crate::ReportCategory::{Changed, NeedsAttention, NotSaved};
    let mut doc = doc_with_a_layer((10, 10));
    let past = rect_sel(-2, -2, 5, 5);
    let outside = rect_sel(60, 60, 2, 2);
    let t = rect_sel(1, 1, 2, 2);
    let (_, outcome) = write_with(
        &mut doc,
        &[("Past", &past), ("Outside", &outside), ("Transparency", &t)],
    );
    let all = outcome.report.items.join("\n");
    assert!(
        all.contains("wholly outside the canvas: \u{201c}Outside\u{201d}"),
        "{all}"
    );
    assert!(all.contains("extend past the canvas"), "{all}");
    assert!(
        all.contains("reserves the channel name Transparency"),
        "{all}"
    );
    assert!(all.contains("hasn't checked them in Photoshop"), "{all}");
    // Task 157: categories and the selection names.
    let cat = |needle: &str| {
        outcome
            .report
            .entries
            .iter()
            .find(|e| e.text.contains(needle))
            .map(|e| (e.category, e.affected.clone()))
    };
    assert_eq!(
        cat("wholly outside the canvas"),
        Some((NotSaved, vec!["Outside".to_owned()]))
    );
    assert_eq!(
        cat("extend past the canvas"),
        Some((Changed, vec!["Past".to_owned()]))
    );
    assert_eq!(
        cat("reserves the channel name Transparency").map(|c| c.0),
        Some(Changed)
    );
    assert_eq!(
        cat("written as alpha channels").map(|c| c.0),
        Some(NeedsAttention)
    );
    // The reserved name round-trips under its new one.
    let (bytes, _) = write_with(&mut doc, &[("Transparency", &t)]);
    assert_eq!(names(&open(&bytes)), ["Transparency 2"]);
}

#[test]
fn verification_catches_a_changed_plane_or_name() {
    let mask = mask_sel(0, 0, 8, 8, |x, y| (x * 9 + y * 5 + 1) as u8);
    let mut doc = doc_with_a_layer((8, 8));
    let (bytes, outcome) = write_with(&mut doc, &[("Sky", &mask)]);
    ok(verify(&bytes, &outcome));
    // A flipped byte in the last plane (the alpha channel's tail).
    let mut bad = bytes.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0x55;
    assert!(verify(&bad, &outcome).is_err());
    // A renamed channel (the Pascal name "Sky").
    let mut bad = bytes.clone();
    let at = bad.windows(3).position(|w| w == b"Sky").unwrap_or(0);
    bad[at] = b'F';
    assert!(verify(&bad, &outcome).is_err());
    // An expectation that forgets the channel.
    let mut forgot = outcome.clone();
    forgot.expected.alpha.clear();
    assert!(verify(&bytes, &forgot).is_err());
}

// ---------------------------------------------------------------------
// Real Photoshop files
// ---------------------------------------------------------------------

fn corpus(path: &str) -> Option<Vec<u8>> {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures")
        .join(path);
    if full.exists() {
        return Some(ok(std::fs::read(&full)));
    }
    println!("SKIPPED: {} is not present", full.display());
    assert!(
        std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
        "AURORA_REQUIRE_CORPUS is set and {path} is missing"
    );
    None
}

#[test]
fn photoshops_own_alpha_channels_open_as_saved_selections() {
    // A plain RGB file with one alpha channel and no layers.
    if let Some(bytes) = corpus("colormodes/4x4_8bit_rgba.psd") {
        // Its Alpha 1 is all black: nothing selected, said so.
        let doc = open(&bytes);
        assert!(doc.saved_selections.is_empty());
        assert!(
            doc.report
                .items
                .iter()
                .any(|i| i.contains("nothing selected")),
            "{:?}",
            doc.report.items
        );
    }
    // A layered file: the Transparency plane comes first and is not an alpha.
    if let Some(bytes) = corpus("effects/stroke-without-vector-mask.psd") {
        let doc = open(&bytes);
        assert_eq!(names(&doc), ["Alpha 1"]);
        assert!(!doc.report.items.iter().any(|i| i.contains("not opened")));
    }
    // Spot plus alpha in a CMYK file.
    if let Some(bytes) = corpus("cmyk-alpha-spot.psd") {
        let doc = open(&bytes);
        assert_eq!(names(&doc), ["Alpha 1"]);
        assert!(
            doc.report
                .items
                .iter()
                .any(|i| i.contains("spot colour channel"))
        );
    }
    // A 32-bit layerless file whose only extra plane is its Transparency.
    if let Some(bytes) = corpus("transparentbg.psd") {
        assert!(open(&bytes).saved_selections.is_empty());
    }
    if let Some(bytes) = corpus("colormodes/4x4_16bit_multichannel.psd") {
        assert!(open(&bytes).saved_selections.is_empty());
    }
    if let Some(bytes) = corpus("blend-modes/normal.psd") {
        assert!(open(&bytes).saved_selections.is_empty());
    }
}

#[test]
fn a_corpus_alpha_channel_survives_open_save_reopen() {
    let Some(bytes) = corpus("colormodes/4x4_8bit_rgba.psd") else {
        return;
    };
    let first = open(&bytes);
    let mut doc = doc_with_a_layer((4, 4));
    let saved: Vec<(&str, &Selection)> = first
        .saved_selections
        .iter()
        .map(|(n, s)| (n.as_str(), s))
        .collect();
    let (out, outcome) = write_with(&mut doc, &saved);
    ok(verify(&out, &outcome));
    let second = open(&out);
    assert_eq!(first.saved_selections, second.saved_selections);
}

/// FNV-1a of a file without its image-resources section (the sRGB profile
/// in it carries lcms2's time stamp).
fn stable_hash(bytes: &[u8]) -> u64 {
    let rl = be32(bytes, 30) as usize;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes[..34].iter().chain(&bytes[34 + rl..]) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[test]
fn a_document_without_saved_selections_writes_the_bytes_main_wrote() {
    // The hashes were taken from the code as of 0.290.0, before alpha
    // channels existed (`git worktree` of c943536).
    for (canvas, len, hash) in [
        ((9u32, 7u32), 1644, 0x8a14_7809_3a33_3803_u64),
        ((16, 12), 3201, 0xfd30_0ff4_c536_43b2),
    ] {
        let mut doc = doc_with_a_layer(canvas);
        let (bytes, _) = ok(doc.write());
        assert_eq!(
            (bytes.len(), stable_hash(&bytes)),
            (len, hash),
            "{canvas:?}"
        );
        let (again, _) = write_with(&mut doc, &[]);
        assert_eq!(stable_hash(&again), hash);
    }
}

#[test]
fn extra_channels_without_any_alpha_resource_stay_unopened_as_before() {
    let doc = open(&flat(6, 4, 8, 0, vec![ramp()], Vec::new()));
    assert!(doc.saved_selections.is_empty());
    assert!(
        doc.report
            .items
            .iter()
            .any(|i| i.contains("1 saved channel")),
        "{:?}",
        doc.report.items
    );
}

/// Writes `alpha.psd` and `alpha.json` for `scripts/verify_psd_write.py`
/// (`check_alpha`): a 16 x 12 document with a rectangle, an ellipse, an
/// inverted rectangle and a fractional mask saved as alpha channels. The
/// JSON holds each channel's name and its 8-bit coverage, row-major.
///
/// ```text
/// AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
///     alpha_fixtures_for_psd_tools -- --ignored
/// ```
#[test]
#[ignore = "writes fixtures for scripts/verify_psd_write.py"]
fn alpha_fixtures_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let (w, h) = (16u32, 12u32);
    let rect = rect_sel(2, 1, 9, 6);
    let ellipse = Selection::ellipse(Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    });
    let mut inverted = rect_sel(4, 3, 5, 4);
    inverted.inverted = true;
    let mask = mask_sel(3, 2, 10, 8, |x, y| ((x * 25 + y * 7) % 256) as u8);
    let saved = [
        ("Box", &rect),
        ("Disc", &ellipse),
        ("Everything but", &inverted),
        ("Gradient", &mask),
    ];
    let mut doc = doc_with_a_layer((w, h));
    let (bytes, outcome) = write_with(&mut doc, &saved);
    ok(verify(&bytes, &outcome));
    let dir = std::path::Path::new(&out);
    ok(std::fs::write(dir.join("alpha.psd"), &bytes));
    let channels: Vec<String> = saved
        .iter()
        .map(|(name, sel)| {
            let cov: Vec<String> = coverage(sel, w, h).iter().map(u8::to_string).collect();
            format!("{{\"name\": {name:?}, \"coverage\": [{}]}}", cov.join(","))
        })
        .collect();
    let json = format!(
        "{{\"width\": {w}, \"height\": {h}, \"channels\": [{}]}}",
        channels.join(",")
    );
    ok(std::fs::write(dir.join("alpha.json"), json));
    // The same selections read back from a real Photoshop-style file list.
    let back = open(&bytes);
    assert_eq!(back.saved_selections.len(), 4);
}

// ---------------------------------------------------------------------
// Review fixes
// ---------------------------------------------------------------------

#[test]
fn a_small_zip_merged_image_claiming_huge_planes_is_refused_before_inflating() {
    use std::io::Write;
    // 1 MiB of zeros deflates to about a kilobyte; the header claims
    // 64 Mpx planes, so the planes could never fit in the payload.
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    ok(enc.write_all(&vec![0u8; 1 << 20]));
    let payload = ok(enc.finish());
    let mut file = vec![0u8; 8];
    file.extend_from_slice(&2u16.to_be_bytes());
    file.extend_from_slice(&payload);
    let mut src = ok(super::StreamSource::new(Cursor::new(file.clone())));
    let window = super::Window {
        pos: 8,
        end: file.len() as u64,
    };
    let header = super::Header {
        version: 1,
        channels: 8,
        color_mode: 3,
        width: 8000,
        height: 8000,
        depth: 8,
    };
    let mut bodies = alpha::ResourceBodies::default();
    bodies.offer(1006, &[1, b'a', 1, b'b', 1, b'c', 1, b'd', 1, b'e']);
    let started = std::time::Instant::now();
    let outcome = ok(read_channels_with(
        &mut src,
        window,
        header,
        3,
        3,
        &bodies,
        Caps::DEFAULT,
    ));
    assert!(started.elapsed().as_secs() < 2);
    assert!(outcome.selections.is_empty());
    assert_eq!(outcome.unreadable, 5);
    // Only the two-byte compression field was read: nothing was inflated.
    assert_eq!(src.total_read, 2);
}

#[test]
fn kind_zero_channels_are_reported_as_inverted_back() {
    let doc = one_alpha(
        8,
        0,
        ramp().iter().map(|b| 255 - b).collect(),
        KIND_INVERTED,
    );
    assert!(
        doc.report.items.iter().any(|i| i.contains(
            "1 alpha channel was stored as inverted (Photoshop's \"selected areas\" option) and was inverted back"
        ) && i.contains("check it")),
        "{:?}",
        doc.report.items
    );
    let plain = one_alpha(8, 0, ramp(), KIND_PLAIN);
    assert!(!plain.report.items.iter().any(|i| i.contains("inverted")));
}

/// A layered file (negative layer count: the merged transparency plane
/// comes first) with one alpha channel of kind `kind`, whose resource lists
/// are `listed`.
fn layered_with_lists(listed: &[ResourceChannel]) -> PsdDocument {
    use super::test_writer::TestLayer;
    let white = vec![0xFF; 24];
    let plane = ramp().iter().map(|b| 255 - b).collect::<Vec<u8>>();
    open(
        &TestPsd::new(1, 6, 4, 8)
            .with(|f| {
                f.channels = 5;
                f.negative_count = true;
                f.layers = vec![TestLayer::pixels("a", 0, 0, 1, 1, 8, &[[0, 0, 0, 255]])];
                f.merged = Some(vec![
                    white.clone(),
                    white.clone(),
                    white.clone(),
                    white,
                    plane,
                ]);
                f.resources = resources(listed);
            })
            .write(),
    )
}

#[test]
fn resource_lists_with_or_without_a_transparency_entry_map_to_the_right_plane() {
    // Photoshop's way: Transparency first, then the alpha (inverted, kind 0).
    let with = layered_with_lists(&[
        ResourceChannel::transparency(),
        channel("Mine", KIND_INVERTED),
    ]);
    // A writer that lists only the alpha channels.
    let without = layered_with_lists(&[channel("Mine", KIND_INVERTED)]);
    for doc in [&with, &without] {
        assert_eq!(names(doc), ["Mine"], "{:?}", doc.report.items);
        // kind 0 was applied to the alpha, not shifted onto Transparency.
        assert_eq!(coverage(&doc.saved_selections[0].1, 6, 4), ramp());
    }
}
