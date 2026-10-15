//! Tests for the Gradient Map (`grdm`) reader and writer and the Color
//! Lookup (`clrL`) finding (0.248.0): Photoshop's own blocks byte for byte
//! (from `layers/gradient-map*.psd`, `layers/color-lookup.psd` and
//! `fill_adjustments.psd`), every layout field, hostile blocks, the report
//! lines, the writer's read-back and rounding, and the corpus files (which
//! print `SKIPPED` when absent). **No fixture holds a picture a Gradient Map
//! was applied to**, so nothing here compares pixels with Photoshop.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::trivially_copy_pass_by_ref,
    clippy::too_many_lines
)]

use aurora_core::{GRADIENT_MAP_MAX_STOPS, GradientMapParams, GradientStop};
use aurora_doc::Adjustment;

use super::read;
use super::read_adjust::{self, Parsed};
use super::read_adjust_tests::{adjustments_of, children_count, corpus, file_with, unhex};
use super::write::{PsdSaveOptions, psd_save_preflight};
use super::write_adjust::encode_adjustment;
use super::write_tests::{Doc, ok, read_back, rect, save};

/// `layers/gradient-map.psd`: version 1, red to white (164 bytes).
const GRDM_V1: &str = "00010000000000190046006f0072006500670072006f0075006e006400200074006f0020004200610063006b00670072006f0075006e00640000000200000000000000320000ffff000000000000000000001000000000320000ffffffffffff000000000002000000000000003200ff000010000000003200ff0002100000200000293a5138000000010000080000030000000000000000800080008000800000000000";

/// `layers/color-lookup.psd` and `fill_adjustments.psd`: an empty descriptor.
const CLRL_EMPTY: &str = "000100000010000000010000000000006e756c6c00000000";

/// The version 3 form of [`GRDM_V1`] with `method` after the flags.
fn v3(method: &[u8; 4]) -> Vec<u8> {
    let v1 = unhex(GRDM_V1);
    let mut v = vec![0, 3];
    v.extend_from_slice(&v1[2..4]);
    v.extend_from_slice(method);
    v.extend_from_slice(&v1[4..]);
    v
}

fn push16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn push32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// A synthetic block in the documented layout.
#[derive(Clone)]
struct Spec {
    version: u16,
    method: [u8; 4],
    reverse: u8,
    dither: u8,
    /// `(location, midpoint, mode, [r, g, b, pad])`.
    colours: Vec<(u32, u32, u16, [u16; 4])>,
    /// `(location, midpoint, opacity)`.
    transparency: Vec<(u32, u32, u16)>,
    expansion: u16,
    smoothness: u16,
    length: u16,
    mode: u16,
    tail: Vec<u8>,
}

impl Spec {
    fn new() -> Self {
        Self {
            version: 1,
            method: *b"Gcls",
            reverse: 0,
            dither: 0,
            colours: vec![
                (0, 50, 0, [65535, 0, 0, 0]),
                (4096, 50, 0, [0, 0, 65535, 0]),
            ],
            transparency: vec![(0, 50, 255), (4096, 50, 255)],
            expansion: 2,
            smoothness: 4096,
            length: 32,
            mode: 0,
            tail: vec![],
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        push16(&mut v, self.version);
        v.push(self.reverse);
        v.push(self.dither);
        if self.version == 3 {
            v.extend_from_slice(&self.method);
        }
        push32(&mut v, 1);
        push16(&mut v, 0);
        push16(&mut v, self.colours.len() as u16);
        for (loc, mid, mode, c) in &self.colours {
            push32(&mut v, *loc);
            push32(&mut v, *mid);
            push16(&mut v, *mode);
            for x in c {
                push16(&mut v, *x);
            }
            push16(&mut v, 0);
        }
        push16(&mut v, self.transparency.len() as u16);
        for (loc, mid, op) in &self.transparency {
            push32(&mut v, *loc);
            push32(&mut v, *mid);
            push16(&mut v, *op);
        }
        push16(&mut v, self.expansion);
        push16(&mut v, self.smoothness);
        push16(&mut v, self.length);
        push16(&mut v, self.mode);
        v.extend_from_slice(&[0; 4 + 2 + 2 + 4 + 2 + 16 + 2]);
        v.extend_from_slice(&self.tail);
        v
    }
}

fn parsed(key: &[u8; 4], data: &[u8]) -> Parsed {
    match read_adjust::parse_block(*key, data) {
        Some(p) => p,
        None => unreachable!("a known key"),
    }
}

fn read_one(key: &[u8; 4], data: &[u8], gray: bool) -> Result<Adjustment, String> {
    read_adjust::adjustment(&parsed(key, data), gray).map_err(|n| format!("{n:?}"))
}

fn map_of(a: Result<Adjustment, String>) -> GradientMapParams {
    match a {
        Ok(Adjustment::GradientMap(p)) => p,
        other => unreachable!("{other:?}"),
    }
}

fn gradient(stops: &[(f32, [f32; 3])], reverse: bool, dither: bool) -> Adjustment {
    let stops = stops
        .iter()
        .map(|&(p, c)| GradientStop::new(p, c))
        .collect();
    Adjustment::GradientMap(ok(GradientMapParams::new(stops, reverse, dither)))
}

// ---------------------------------------------------------------------
// Reading Photoshop's own blocks
// ---------------------------------------------------------------------

#[test]
fn photoshops_version_one_block_is_red_to_white() {
    assert_eq!(unhex(GRDM_V1).len(), 164);
    let p = map_of(read_one(b"grdm", &unhex(GRDM_V1), false));
    assert_eq!(p.stops().len(), 2);
    assert_eq!(p.stops()[0], GradientStop::new(0.0, [1.0, 0.0, 0.0]));
    assert_eq!(p.stops()[1], GradientStop::new(1.0, [1.0, 1.0, 1.0]));
    assert!(!p.reverse() && !p.dither());
}

#[test]
fn the_version_three_classic_block_reads_like_version_one_and_the_other_methods_are_left_out() {
    assert_eq!(v3(b"Gcls").len(), 168);
    assert_eq!(
        map_of(read_one(b"grdm", &v3(b"Gcls"), false)),
        map_of(read_one(b"grdm", &unhex(GRDM_V1), false))
    );
    for method in [b"Lnr ", b"Perc", b"Smth", b"abcd"] {
        assert_eq!(
            read_one(b"grdm", &v3(method), false),
            Err("GradientLeftOut(Method)".to_owned()),
            "{method:?}"
        );
    }
}

#[test]
fn flags_are_read_from_their_bytes() {
    for (reverse, dither) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let mut s = Spec::new();
        s.reverse = reverse;
        s.dither = dither;
        let p = map_of(read_one(b"grdm", &s.bytes(), false));
        assert_eq!((p.reverse(), p.dither()), (reverse == 1, dither == 1));
    }
    for bad in [2, 255] {
        let mut s = Spec::new();
        s.reverse = bad;
        assert_eq!(
            read_one(b"grdm", &s.bytes(), false),
            Err("Unreadable(GradientMap)".to_owned())
        );
        let mut s = Spec::new();
        s.dither = bad;
        assert!(read_one(b"grdm", &s.bytes(), false).is_err());
    }
}

#[test]
fn positions_and_colours_are_scaled_by_4096_and_65535() {
    let mut s = Spec::new();
    s.colours = vec![
        (1024, 50, 0, [0x7b7a, 0x5b08, 0x5b08, 0]),
        (3072, 50, 0, [65535, 32768, 1, 0]),
    ];
    let p = map_of(read_one(b"grdm", &s.bytes(), false));
    assert_eq!(p.stops()[0].position, 0.25);
    assert_eq!(p.stops()[1].position, 0.75);
    assert_eq!(p.stops()[0].colour[0], f32::from(0x7b7a_u16) / 65535.0);
    assert_eq!(p.stops()[1].colour[1], 32768.0 / 65535.0);
    assert_eq!(p.stops()[1].colour[2], 1.0 / 65535.0);
}

#[test]
fn what_the_model_cannot_hold_is_left_out_with_its_own_reason() {
    let reason = |s: &Spec| match read_one(b"grdm", &s.bytes(), false) {
        Err(note) => note,
        Ok(a) => unreachable!("expected a note, got {a:?}"),
    };
    let mut s = Spec::new();
    s.colours[0].1 = 49;
    assert_eq!(reason(&s), "GradientLeftOut(Midpoint)");
    let mut s = Spec::new();
    s.colours[1].1 = 51; // the last stop too
    assert_eq!(reason(&s), "GradientLeftOut(Midpoint)");
    let mut s = Spec::new();
    s.transparency[1].2 = 254;
    assert_eq!(reason(&s), "GradientLeftOut(Transparency)");
    let mut s = Spec::new();
    s.colours[1].2 = 2; // a CMYK stop
    assert_eq!(reason(&s), "GradientLeftOut(ColourMode)");
    let mut s = Spec::new();
    s.mode = 1;
    assert_eq!(reason(&s), "GradientLeftOut(Noise)");
    let mut s = Spec::new();
    s.colours = (0..=GRADIENT_MAP_MAX_STOPS as u32)
        .map(|i| (i * 60, 50, 0, [1, 1, 1, 0]))
        .collect();
    assert_eq!(s.colours.len(), 65);
    assert_eq!(reason(&s), "GradientLeftOut(Stops)");
    s.colours.pop();
    assert!(
        read_one(b"grdm", &s.bytes(), false).is_ok(),
        "64 stops import"
    );
}

#[test]
fn out_of_range_or_inconsistent_fields_are_unreadable_not_repaired() {
    let unreadable = |f: &dyn Fn(&mut Spec)| {
        let mut s = Spec::new();
        f(&mut s);
        assert_eq!(
            read_one(b"grdm", &s.bytes(), false),
            Err("Unreadable(GradientMap)".to_owned())
        );
    };
    unreadable(&|s| s.version = 2);
    unreadable(&|s| s.version = 0);
    unreadable(&|s| s.colours[1].0 = 4097);
    unreadable(&|s| s.colours[0].1 = 101);
    unreadable(&|s| s.colours[0].3[3] = 1);
    unreadable(&|s| s.transparency[0].2 = 256);
    unreadable(&|s| s.smoothness = 4097);
    unreadable(&|s| s.expansion = 3);
    unreadable(&|s| s.length = 31);
    unreadable(&|s| s.mode = 2);
    unreadable(&|s| s.colours.truncate(1));
    unreadable(&|s| s.colours.clear());
    unreadable(&|s| s.colours.swap(0, 1));
    unreadable(&|s| s.tail = vec![0, 0, 0, 0]);
    unreadable(&|s| s.tail = vec![1]);
    // One to three zero bytes are the block's own padding.
    for n in 0..=3 {
        let mut s = Spec::new();
        s.tail = vec![0; n];
        assert!(read_one(b"grdm", &s.bytes(), false).is_ok(), "{n}");
    }
}

#[test]
fn a_grayscale_file_leaves_the_layer_out() {
    assert_eq!(
        read_one(b"grdm", &unhex(GRDM_V1), true),
        Err("Grayscale(GradientMap)".to_owned())
    );
}

#[test]
fn equal_positions_and_reversed_stops_in_the_file_keep_their_meaning() {
    let mut s = Spec::new();
    s.colours = vec![
        (0, 50, 0, [0, 0, 0, 0]),
        (2048, 50, 0, [65535, 0, 0, 0]),
        (2048, 50, 0, [0, 65535, 0, 0]),
        (4096, 50, 0, [65535, 65535, 65535, 0]),
    ];
    s.reverse = 1;
    let p = map_of(read_one(b"grdm", &s.bytes(), false));
    assert_eq!(p.stops().len(), 4);
    assert_eq!(p.stops()[1].position, p.stops()[2].position);
    assert!(p.reverse());
}

// ---------------------------------------------------------------------
// Hostile blocks
// ---------------------------------------------------------------------

#[test]
fn truncated_and_hostile_blocks_never_panic_and_allocate_little() {
    let good = unhex(GRDM_V1);
    // The last two bytes are the block's own padding; every shorter prefix
    // is an error.
    for cut in 0..good.len() - 2 {
        let r = read_adjust::adjustment(&parsed(b"grdm", &good[..cut]), false);
        assert!(r.is_err(), "prefix {cut}");
    }
    for cut in 0..v3(b"Gcls").len() {
        let _ = read_adjust::adjustment(&parsed(b"grdm", &v3(b"Gcls")[..cut]), false);
    }
    // A name length of four billion units, a colour stop count of 65,535 in
    // a 30-byte block, a transparency count of 65,535, and a huge block.
    let mut huge_name = vec![0, 1, 0, 0];
    huge_name.extend_from_slice(&u32::MAX.to_be_bytes());
    huge_name.extend_from_slice(&[0; 32]);
    let mut many_colours = vec![0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0xff, 0xff];
    many_colours.extend_from_slice(&[0; 30]);
    let mut many_transparency = Spec::new().bytes();
    let at = 4 + 4 + 2 + 2 + 2 * 20; // after the two colour stops' count
    many_transparency[at] = 0xff;
    many_transparency[at + 1] = 0xff;
    for block in [
        huge_name,
        many_colours,
        many_transparency,
        vec![0; 70_000],
        vec![],
        vec![0xff; 5000],
    ] {
        assert!(
            read_adjust::adjustment(&parsed(b"grdm", &block), false).is_err(),
            "{} bytes",
            block.len()
        );
    }
    // Pseudo-random bytes of many lengths: totality only.
    let mut x = 0x1234_5678_u32;
    for len in (0..400).chain([1000, 4000, 65_536]) {
        let block: Vec<u8> = (0..len)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        let _ = read_adjust::adjustment(&parsed(b"grdm", &block), false);
        let _ = read_adjust::adjustment(&parsed(b"clrL", &block), false);
    }
}

// ---------------------------------------------------------------------
// Whole-file import and the report lines
// ---------------------------------------------------------------------

fn items_of(key: &[u8; 4], block: &[u8]) -> (String, usize, Vec<(String, Adjustment)>) {
    let document = ok(read(&file_with(key, block, |_| {}).write()));
    (
        document.report.items.join("\n"),
        children_count(&document),
        adjustments_of(&document),
    )
}

#[test]
fn a_gradient_map_opens_as_a_layer_with_an_honest_unvalidated_line() {
    let (items, children, found) = items_of(b"grdm", &unhex(GRDM_V1));
    assert_eq!(children, 2, "the pixel layer and the adjustment");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.label(), "Gradient Map");
    assert!(
        items.contains("1 Gradient Map layer is shown with Classic colour interpolation"),
        "{items}"
    );
    assert!(
        items.contains("has not been compared with Photoshop"),
        "{items}"
    );
    assert!(!items.contains("smoothness"), "{items}");
}

#[test]
fn a_smoothness_below_one_hundred_percent_is_drawn_smooth_and_said_so() {
    let mut s = Spec::new();
    s.smoothness = 2048;
    let (items, _, found) = items_of(b"grdm", &s.bytes());
    assert_eq!(found.len(), 1);
    assert!(
        items.contains("1 Gradient Map layer has a smoothness below 100 %"),
        "{items}"
    );
}

#[test]
fn left_out_gradient_maps_get_their_own_report_lines_and_no_layer() {
    for (block, text) in [
        (v3(b"Perc"), "Linear or Perceptual colour interpolation"),
        (
            {
                let mut s = Spec::new();
                s.colours[0].1 = 10;
                s.bytes()
            },
            "midpoint is not the default 50 %",
        ),
        (
            {
                let mut s = Spec::new();
                s.transparency[0].2 = 0;
                s.bytes()
            },
            "transparency stops below full opacity",
        ),
        (
            {
                let mut s = Spec::new();
                s.mode = 1;
                s.bytes()
            },
            "Noise gradient",
        ),
        (vec![0; 4], "settings could not be read"),
    ] {
        let (items, children, found) = items_of(b"grdm", &block);
        assert!(found.is_empty(), "{text}");
        assert_eq!(children, 1, "only the pixel layer: {text}");
        assert!(
            items.contains("1 Gradient Map layer was left out"),
            "{items}"
        );
        assert!(items.contains(text), "{text}: {items}");
        assert!(!items.contains("shown with Classic"), "{items}");
    }
}

#[test]
fn the_generic_left_out_line_no_longer_claims_gradient_map_or_color_lookup() {
    let document = ok(read(
        &file_with(
            b"mixr",
            &unhex(super::mixer_selective_tests::MIXR_NEUTRAL_FOR_TESTS),
            |l| {
                l.blocks.push((*b"hue ", vec![0; 4]));
            },
        )
        .write(),
    ));
    let items = document.report.items.join("\n");
    assert!(
        items.contains("1 adjustment layer (of a kind Aurora does not know)"),
        "{items}"
    );
    assert!(!items.contains("Gradient Map, Color Lookup"), "{items}");
    assert!(
        items.contains("Selective Color and Gradient Map adjustment"),
        "{items}"
    );
}

// ---------------------------------------------------------------------
// Color Lookup: left out, and why
// ---------------------------------------------------------------------

/// A `clrL` block holding one `Nm  ` text item.
fn clrl_named(name: &str) -> Vec<u8> {
    let mut v = unhex("0001000000100000000100000000000000000000");
    v.truncate(6);
    push32(&mut v, 1); // empty unicode name: one unit
    push16(&mut v, 0);
    push32(&mut v, 0);
    v.extend_from_slice(b"null");
    push32(&mut v, 1); // one item
    push32(&mut v, 0);
    v.extend_from_slice(b"Nm  ");
    v.extend_from_slice(b"TEXT");
    let units: Vec<u16> = name.encode_utf16().chain([0]).collect();
    push32(&mut v, units.len() as u32);
    for u in units {
        push16(&mut v, u);
    }
    v
}

#[test]
fn photoshops_color_lookup_block_is_an_empty_descriptor_with_no_table() {
    assert_eq!(unhex(CLRL_EMPTY).len(), 24);
    let Parsed::ColorLookup(Ok(block)) = parsed(b"clrL", &unhex(CLRL_EMPTY)) else {
        unreachable!("readable")
    };
    assert!(block.empty && block.name.is_none());
    let (items, children, found) = items_of(b"clrL", &unhex(CLRL_EMPTY));
    assert!(found.is_empty());
    assert_eq!(children, 1);
    assert!(
        items.contains("1 Color Lookup layer was left out — it holds no lookup table"),
        "{items}"
    );
}

#[test]
fn a_color_lookup_with_a_table_is_left_out_and_its_name_is_reported() {
    let block = clrl_named("Fuji\u{7}Film\nLook");
    let Parsed::ColorLookup(Ok(parsed_block)) = parsed(b"clrL", &block) else {
        unreachable!("readable")
    };
    assert!(!parsed_block.empty);
    assert_eq!(
        parsed_block.name.as_deref(),
        Some("FujiFilmLook"),
        "controls dropped"
    );
    let (items, children, found) = items_of(b"clrL", &block);
    assert!(found.is_empty() && children == 1);
    assert!(
        items.contains("1 Color Lookup layer was left out — it uses a lookup table"),
        "{items}"
    );
    assert!(
        items.contains("Named: \u{201c}FujiFilmLook\u{201d}."),
        "{items}"
    );
    assert!(
        read_one(b"clrL", &block, false).is_err() && read_one(b"clrL", &block, true).is_err(),
        "never becomes a layer, in colour or grey"
    );
}

#[test]
fn a_very_long_lookup_name_is_capped_and_garbage_is_unreadable() {
    let long = "x".repeat(500);
    let Parsed::ColorLookup(Ok(b)) = parsed(b"clrL", &clrl_named(&long)) else {
        unreachable!("readable")
    };
    assert_eq!(b.name.map(|n| n.chars().count()), Some(64));
    let (items, ..) = items_of(b"clrL", &[0, 1, 0xde, 0xad]);
    assert!(
        items.contains("1 Color Lookup layer was left out — its settings could not be read"),
        "{items}"
    );
    assert!(read_one(b"clrL", &[], false).is_err());
    let mut bad_version = unhex(CLRL_EMPTY);
    bad_version[1] = 2;
    assert_eq!(
        read_one(b"clrL", &bad_version, false),
        Err("ColorLookup(Unreadable)".to_owned())
    );
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

fn encoded(a: &Adjustment) -> super::write_adjust::EncodedAdjustment {
    match encode_adjustment(a) {
        Ok(e) => e,
        Err(reason) => unreachable!("{a:?}: {reason}"),
    }
}

#[test]
fn the_writer_reads_back_as_itself_and_uses_photoshops_fixed_fields() {
    let a = gradient(
        &[
            (0.0, [0.0, 0.0, 0.0]),
            (0.25, [1.0, 0.0, 0.0]),
            (0.5, [0.0, 1.0, 0.0]),
            (0.5, [0.0, 0.0, 1.0]),
            (1.0, [1.0, 1.0, 1.0]),
        ],
        true,
        true,
    );
    let e = encoded(&a);
    assert_eq!(e.key, *b"grdm");
    assert_eq!(e.readback, a);
    assert!(!e.rounding.rounded);
    assert_eq!(e.data.len() % 4, 0);
    // Version 1, then Reverse and Dither.
    assert_eq!(&e.data[..4], &[0, 1, 1, 1]);
    // The same trailer as Photoshop's own file: everything after the last
    // colour stop's end is the transparency stops and the fixed settings.
    let own = unhex(GRDM_V1);
    // Photoshop's block ends with the transparency stops (a count and two
    // 10-byte stops) and 40 bytes of settings: 62 bytes, then padding to
    // four (2 bytes in the original, 2 in ours). Ours is identical.
    let last = |d: &[u8]| d[d.len() - 2 - 62..d.len() - 2].to_vec();
    assert_eq!(
        last(&e.data),
        last(&own),
        "transparency stops and settings match Photoshop's block"
    );
}

#[test]
fn photoshops_own_block_survives_read_write_read_with_its_values() {
    let p = map_of(read_one(b"grdm", &unhex(GRDM_V1), false));
    let again = encoded(&Adjustment::GradientMap(p.clone()));
    assert_eq!(again.readback, Adjustment::GradientMap(p));
    assert!(!again.rounding.rounded, "k/4096 and k/65535 are exact");
    // fill_adjustments.psd's first stop colour is exact too.
    let mut s = Spec::new();
    s.colours[0].3 = [0x7b7a, 0x5b08, 0x5b08, 0];
    let p = map_of(read_one(b"grdm", &s.bytes(), false));
    assert!(!encoded(&Adjustment::GradientMap(p)).rounding.rounded);
}

#[test]
fn positions_and_colours_off_the_files_grid_are_rounded_and_itemised() {
    let off_position = gradient(
        &[(0.0, [0.0; 3]), (0.3, [0.5; 3]), (1.0, [1.0; 3])],
        false,
        false,
    );
    let e = encoded(&off_position);
    assert!(e.rounding.rounded);
    let Adjustment::GradientMap(back) = e.readback else {
        unreachable!("gradient")
    };
    assert_eq!(back.stops()[1].position, 1229.0 / 4096.0);
    let off_colour = gradient(&[(0.0, [0.1, 0.2, 0.3]), (1.0, [1.0; 3])], false, false);
    assert!(encoded(&off_colour).rounding.rounded);
    let mut tree = aurora_doc::LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "G",
        gradient(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], false, false),
        None,
        0,
    ));
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    assert!(report.items.is_empty(), "{:?}", report.items);
    ok(tree.add_adjustment_layer_at("G2", off_colour, None, 0));
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    assert!(
        report.items.join("\n").contains(
            "1 Gradient Map layer's stop positions will be rounded to 4096ths and its colours to 65,535ths."
        ),
        "{:?}",
        report.items
    );
}

#[test]
fn a_gradient_map_round_trips_through_write_verify_and_read() {
    let c = |k: u16| f32::from(k) / 65535.0;
    let mut doc = Doc::new((16, 16));
    doc.layer("Background", rect(0, 0, 16, 16), None);
    let g = ok(doc.tree.add_adjustment_layer_at(
        "Duotone",
        gradient(
            &[
                (0.0, [0.0, 0.0, c(16384)]),
                (0.5, [c(49152), c(32768), 0.0]),
                (1.0, [1.0, 1.0, c(32768)]),
            ],
            true,
            false,
        ),
        None,
        0,
    ));
    ok(doc.tree.set_opacity(g, 128.0 / 255.0));
    ok(doc.tree.set_blend_mode(g, aurora_doc::BlendMode::Multiply));
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("g.psd");
    let outcome = ok(save(&mut doc, &target, false));
    assert!(
        outcome.report.items.is_empty(),
        "{:?}",
        outcome.report.items
    );
    let bytes = ok(std::fs::read(&target));
    let document = read_back(&bytes);
    let found = adjustments_of(&document);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "Duotone");
    assert_eq!(
        found[0].1,
        doc.tree
            .adjustment(g)
            .cloned()
            .unwrap_or(Adjustment::Invert)
    );
    let id = document
        .layers
        .roots()
        .iter()
        .copied()
        .find(|&i| document.layers.name(i) == Some("Duotone"));
    let Some(id) = id else { unreachable!("layer") };
    assert_eq!(
        document.layers.blend_mode(id),
        Some(aurora_doc::BlendMode::Multiply)
    );
}

// ---------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------

#[test]
fn corpus_gradient_maps_import_and_color_lookups_are_left_out() {
    for (file, method_ok) in [
        ("layers/gradient-map.psd", true),
        ("layers/gradient-map-v3-classic.psd", true),
        ("layers/gradient-map-v3-linear.psd", false),
        ("layers/gradient-map-v3-perceptual.psd", false),
    ] {
        let Some(document) = corpus(file) else {
            continue;
        };
        let found = adjustments_of(&document);
        let items = document.report.items.join("\n");
        if method_ok {
            assert_eq!(found.len(), 1, "{file}: {items}");
            let p = map_of(Ok(found[0].1.clone()));
            assert_eq!(p.stops()[0].colour, [1.0, 0.0, 0.0], "{file}");
            assert_eq!(p.stops()[1].colour, [1.0; 3], "{file}");
            assert!(items.contains("shown with Classic"), "{file}: {items}");
        } else {
            assert!(found.is_empty(), "{file}");
            assert!(items.contains("Linear or Perceptual"), "{file}: {items}");
        }
    }
    if let Some(document) = corpus("layers/color-lookup.psd") {
        assert!(adjustments_of(&document).is_empty());
        let items = document.report.items.join("\n");
        assert!(items.contains("holds no lookup table"), "{items}");
    }
    if let Some(document) = corpus("fill_adjustments.psd") {
        let found = adjustments_of(&document);
        let gradient = found.iter().find(|(_, a)| a.label() == "Gradient Map");
        let Some((_, Adjustment::GradientMap(p))) = gradient else {
            unreachable!("fill_adjustments holds a Gradient Map");
        };
        assert_eq!(p.stops()[0].colour[0], f32::from(0x7b7a_u16) / 65535.0);
        assert!(found.iter().all(|(_, a)| a.label() != "Color Lookup"));
        let items = document.report.items.join("\n");
        assert!(
            items.contains("1 Color Lookup layer was left out"),
            "{items}"
        );
        assert!(!items.contains("2 adjustment layers"), "{items}");
    }
}

#[test]
fn the_neutral_check_values_are_unused() {
    // Keeps `GRADIENT_MAP_MAX_STOPS` in the public API under test.
    assert_eq!(GRADIENT_MAP_MAX_STOPS, 64);
}

/// Writes the fixture `scripts/verify_psd_write.py` reads (psd-tools is the
/// independent reader):
///
/// ```text
/// AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
///     gradient_map_fixture_for_psd_tools -- --ignored
/// ```
#[test]
#[ignore = "writes the fixture scripts/verify_psd_write.py reads"]
fn gradient_map_fixture_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = std::path::Path::new(&out);
    let mut doc = Doc::new((16, 16));
    doc.layer("Background", rect(0, 0, 16, 16), None);
    let c = |k: u16| f32::from(k) / 65535.0;
    let g = ok(doc.tree.add_adjustment_layer_at(
        "Map",
        gradient(
            &[
                (0.0, [0.0, 0.0, c(16384)]),
                (0.5, [c(49152), c(32768), 0.0]),
                (1.0, [1.0, 1.0, c(32768)]),
            ],
            true,
            true,
        ),
        None,
        0,
    ));
    ok(doc.tree.set_opacity(g, 128.0 / 255.0));
    ok(doc.tree.set_blend_mode(g, aurora_doc::BlendMode::Multiply));
    let (bytes, _) = ok(doc.write());
    ok(std::fs::write(out.join("gradientmap.psd"), &bytes));
    let json = "{\"stops\": [[0, 0, 0, 16384], [2048, 49152, 32768, 0], [4096, 65535, 65535, 32768]], \
        \"reverse\": 1, \"dither\": 1}\n";
    ok(std::fs::write(out.join("gradientmap.json"), json));
}
