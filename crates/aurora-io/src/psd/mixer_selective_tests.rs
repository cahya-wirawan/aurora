//! Tests for the Channel Mixer (`mixr`) and Selective Color (`selc`)
//! layers on the PSD reader and writer (0.245.0): Photoshop's neutral
//! blocks byte for byte, the block layouts, hostile blocks, the report
//! lines, the writer's rounding and read-back, and the corpus files (which
//! print `SKIPPED` when absent). Only the neutral case is Photoshop's own
//! data; every non-neutral block here is synthetic.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use aurora_core::{ChannelMixerParams, SelectiveColorParams};
use aurora_doc::{Adjustment, LayerId, LayerTree};

use super::read;
use super::read_adjust::{self, Parsed};
use super::read_adjust_tests::{
    adjustments_of, children_count, corpus, file_with, top_adjustment, unhex,
};
use super::test_writer::{TestLayer, TestPsd};
use super::write::{PsdSaveOptions, psd_save_preflight};
use super::write_adjust::encode_adjustment;
use super::write_tests::{Doc, ok, read_back, rect, save};
use crate::error::IoError;

/// `layers/channel-mixer.psd` and `fill_adjustments.psd` "Channel Mixer 1":
/// the neutral mixer (version 1, Monochrome 0, four rows of five).
const MIXR_NEUTRAL: &str =
    "0001000000640000000000000000000000640000000000000000000000640000000000000000000000640000";

fn selc_neutral() -> Vec<u8> {
    // `layers/selective-color.psd`: version 1, method 0, ten zero plates.
    let mut v = vec![0, 1, 0, 0];
    v.extend_from_slice(&[0; 80]);
    v
}

fn be16(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn mixr(version: i16, mono: i16, rows: [[i16; 5]; 4]) -> Vec<u8> {
    let mut v = be16(&[version, mono]);
    for row in rows {
        v.extend(be16(&row));
    }
    v
}

fn selc(version: i16, method: i16, plates: [[i16; 4]; 10]) -> Vec<u8> {
    let mut v = be16(&[version, method]);
    for plate in plates {
        v.extend(be16(&plate));
    }
    v
}

const ID_ROWS: [[i16; 5]; 4] = [
    [100, 0, 0, 0, 0],
    [0, 100, 0, 0, 0],
    [0, 0, 100, 0, 0],
    [0, 0, 0, 100, 0],
];

fn parsed(key: &[u8; 4], data: &[u8]) -> Parsed {
    match read_adjust::parse_block(*key, data) {
        Some(p) => p,
        None => unreachable!("a known key"),
    }
}

fn read_one(key: &[u8; 4], data: &[u8], gray: bool) -> Result<Adjustment, String> {
    read_adjust::adjustment(&parsed(key, data), gray).map_err(|n| format!("{n:?}"))
}

fn mixer(rows: [[f32; 4]; 3], mono: bool) -> Adjustment {
    Adjustment::ChannelMixer(ok(ChannelMixerParams::new(rows, mono)))
}

fn selective(set: &[(usize, [f32; 4])], absolute: bool) -> Adjustment {
    let mut ranges = [[0.0_f32; 4]; 9];
    for (i, row) in set {
        ranges[*i] = *row;
    }
    Adjustment::SelectiveColor(ok(SelectiveColorParams::new(ranges, absolute)))
}

// ---------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------

#[test]
fn photoshops_neutral_blocks_parse_to_the_identity_layers() {
    assert_eq!(unhex(MIXR_NEUTRAL).len(), 44);
    assert_eq!(
        read_one(b"mixr", &unhex(MIXR_NEUTRAL), false),
        Ok(Adjustment::ChannelMixer(ChannelMixerParams::identity()))
    );
    assert_eq!(
        read_one(b"selc", &selc_neutral(), false),
        Ok(Adjustment::SelectiveColor(SelectiveColorParams::identity()))
    );
    assert_eq!(selc_neutral().len(), 84);
}

#[test]
fn the_mixer_layout_is_four_rows_of_red_green_blue_unused_constant() {
    let rows = [
        [1, 2, 3, 99, 4],
        [5, 6, 7, 99, 8],
        [-200, 200, -9, 99, -100],
        [0, 0, 0, 100, 0],
    ];
    let Ok(Adjustment::ChannelMixer(p)) = read_one(b"mixr", &mixr(1, 0, rows), false) else {
        unreachable!("readable")
    };
    assert_eq!(
        p.rows(),
        [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [-200.0, 200.0, -9.0, -100.0]
        ],
        "the fourth source is ignored, the fifth value is the constant"
    );
    assert!(!p.monochrome());
    let Ok(Adjustment::ChannelMixer(p)) = read_one(b"mixr", &mixr(1, 1, rows), false) else {
        unreachable!("readable")
    };
    assert!(p.monochrome());
    // A block with a tail (Photoshop pads tagged blocks) still reads.
    let mut longer = mixr(1, 0, ID_ROWS);
    longer.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(
        read_one(b"mixr", &longer, false),
        Ok(Adjustment::ChannelMixer(ChannelMixerParams::identity()))
    );
}

#[test]
fn the_selective_layout_is_a_method_an_unused_plate_and_nine_ranges() {
    let mut plates = [[0_i16; 4]; 10];
    for (i, plate) in plates.iter_mut().enumerate().skip(1) {
        *plate = [i as i16, -(i as i16), 10 * i as i16, -10 * i as i16];
    }
    let Ok(Adjustment::SelectiveColor(p)) = read_one(b"selc", &selc(1, 1, plates), false) else {
        unreachable!("readable")
    };
    assert!(p.absolute());
    for r in 0..9 {
        let i = (r + 1) as f32;
        assert_eq!(p.range(r), Some([i, -i, 10.0 * i, -10.0 * i]), "range {r}");
    }
    let Ok(Adjustment::SelectiveColor(p)) = read_one(b"selc", &selc(1, 0, plates), false) else {
        unreachable!("readable")
    };
    assert!(!p.absolute());
}

#[test]
fn unknown_versions_flags_and_out_of_range_values_are_unreadable_not_repaired() {
    let bad_mixers = [
        mixr(2, 0, ID_ROWS),
        mixr(0, 0, ID_ROWS),
        mixr(1, 2, ID_ROWS),
        mixr(1, -1, ID_ROWS),
        mixr(
            1,
            0,
            [[201, 0, 0, 0, 0], ID_ROWS[1], ID_ROWS[2], ID_ROWS[3]],
        ),
        mixr(
            1,
            0,
            [ID_ROWS[0], [0, -201, 0, 0, 0], ID_ROWS[2], ID_ROWS[3]],
        ),
        mixr(
            1,
            0,
            [ID_ROWS[0], ID_ROWS[1], [0, 0, 100, 0, 101], ID_ROWS[3]],
        ),
        mixr(
            1,
            0,
            [ID_ROWS[0], ID_ROWS[1], [0, 0, 100, 0, -101], ID_ROWS[3]],
        ),
        unhex(MIXR_NEUTRAL)[..43].to_vec(),
        Vec::new(),
    ];
    for (i, data) in bad_mixers.iter().enumerate() {
        let r = read_one(b"mixr", data, false);
        assert!(
            matches!(&r, Err(e) if e.contains("Unreadable") && e.contains("ChannelMixer")),
            "mixer {i}: {r:?}"
        );
    }
    let zero = [[0_i16; 4]; 10];
    let with = |plate: usize, slot: usize, v: i16| {
        let mut p = zero;
        p[plate][slot] = v;
        p
    };
    let bad_selectives = [
        selc(2, 0, zero),
        selc(1, 2, zero),
        selc(1, -1, zero),
        selc(1, 0, with(1, 0, 101)),
        selc(1, 0, with(9, 3, -101)),
        // The unused plate must be zero.
        selc(1, 0, with(0, 0, 1)),
        selc(1, 0, with(0, 3, -1)),
        selc_neutral()[..83].to_vec(),
        Vec::new(),
    ];
    for (i, data) in bad_selectives.iter().enumerate() {
        let r = read_one(b"selc", data, false);
        assert!(
            matches!(&r, Err(e) if e.contains("Unreadable") && e.contains("SelectiveColor")),
            "selective {i}: {r:?}"
        );
    }
    // The boundary values themselves are fine.
    let edge = [[100, -100, 100, -100]; 10];
    let mut edge = edge;
    edge[0] = [0; 4];
    assert!(read_one(b"selc", &selc(1, 0, edge), false).is_ok());
    assert!(
        read_one(
            b"mixr",
            &mixr(
                1,
                0,
                [[200, -200, 200, 0, 100], ID_ROWS[1], ID_ROWS[2], ID_ROWS[3]]
            ),
            false
        )
        .is_ok()
    );
}

#[test]
fn a_grayscale_file_leaves_both_out_with_their_own_note() {
    for (key, data, name) in [
        (b"mixr", unhex(MIXR_NEUTRAL), "Channel Mixer"),
        (b"selc", selc_neutral(), "Selective Color"),
    ] {
        let layer = TestLayer::empty("Adjust 1").with(|l| l.blocks.push((*key, data.clone())));
        let bytes = TestPsd::new(1, 2, 2, 8)
            .with(|p| {
                p.color_mode = 1;
                p.layers = vec![TestLayer::divider(), layer, TestLayer::group("G", *b"pass")];
            })
            .write();
        let document = ok(read(&bytes));
        let items = document.report.items.join("\n");
        assert!(
            items.contains(&format!(
                "1 {name} layer in this Grayscale file was left out"
            )),
            "{name}: {items}"
        );
        assert_eq!(children_count(&document), 0);
    }
}

#[test]
fn hostile_blocks_never_panic_and_allocate_nothing_unbounded() {
    let mut state = 0x1234_5678_u32;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as u8
    };
    for len in (0..200).chain([1_000, 70_000]) {
        for _ in 0..3 {
            let data: Vec<u8> = (0..len).map(|_| next()).collect();
            for key in [b"mixr", b"selc"] {
                let p = parsed(key, &data);
                let _ = read_adjust::adjustment(&p, false);
                let _ = read_adjust::adjustment(&p, true);
            }
        }
    }
    // Plausible headers with garbage after them.
    for key in [b"mixr", b"selc"] {
        let mut data = vec![0, 1, 0, 0];
        data.extend((0..300).map(|_| next()));
        let _ = read_adjust::adjustment(&parsed(key, &data), false);
    }
}

#[test]
fn a_non_neutral_layer_is_reported_as_unvalidated_and_a_neutral_one_is_not() {
    let strong = mixr(1, 0, [[50, 0, 0, 0, 0], ID_ROWS[1], ID_ROWS[2], ID_ROWS[3]]);
    let mut plates = [[0_i16; 4]; 10];
    plates[1] = [10, 0, 0, 0];
    for (key, neutral, strong, name) in [
        (b"mixr", unhex(MIXR_NEUTRAL), strong, "Channel Mixer"),
        (
            b"selc",
            selc_neutral(),
            selc(1, 0, plates),
            "Selective Color",
        ),
    ] {
        let document = ok(read(&file_with(key, &neutral, |_| {}).write()));
        let items = document.report.items.join("\n");
        assert!(!items.contains("textbook"), "{name} neutral: {items}");
        assert!(!items.contains("left out"), "{name} neutral: {items}");
        assert_eq!(children_count(&document), 2, "pixels and the adjustment");
        let document = ok(read(&file_with(key, &strong, |_| {}).write()));
        let items = document.report.items.join("\n");
        assert!(
            items.contains(&format!(
                "1 {name} layer is shown with a textbook formula that has not been compared with Photoshop"
            )),
            "{name}: {items}"
        );
        assert!(top_adjustment(&document).is_some());
    }
    // An unreadable one is left out with its own line, not repaired.
    let document = ok(read(
        &file_with(b"mixr", &mixr(2, 0, ID_ROWS), |_| {}).write(),
    ));
    let items = document.report.items.join("\n");
    assert!(
        items.contains("1 Channel Mixer layer was left out"),
        "{items}"
    );
    assert_eq!(children_count(&document), 1, "only the pixel layer");
}

#[test]
fn the_left_out_note_no_longer_names_the_channel_mixer() {
    let document = ok(read(
        &file_with(b"mixr", &unhex(MIXR_NEUTRAL), |l| {
            l.blocks.push((*b"grdm", vec![0; 4]));
        })
        .write(),
    ));
    let items = document.report.items.join("\n");
    assert!(
        items.contains("1 adjustment layer (Gradient Map, Color Lookup"),
        "{items}"
    );
    assert!(
        items.contains("Channel Mixer and Selective Color adjustment layers"),
        "{items}"
    );
    assert_eq!(
        children_count(&document),
        1,
        "only the pixel layer: the record is left out"
    );
}

#[test]
fn corpus_neutral_layers_import_as_identity_without_a_report_line() {
    for (file, want) in [
        (
            "layers/channel-mixer.psd",
            Adjustment::ChannelMixer(ChannelMixerParams::identity()),
        ),
        (
            "layers/selective-color.psd",
            Adjustment::SelectiveColor(SelectiveColorParams::identity()),
        ),
    ] {
        let Some(document) = corpus(file) else {
            continue;
        };
        let found = adjustments_of(&document);
        assert_eq!(found.len(), 1, "{file}: {found:?}");
        assert_eq!(found[0].1, want, "{file}");
        assert!(
            document.report.items.is_empty(),
            "{file}: {:?}",
            document.report.items
        );
    }
    let Some(document) = corpus("fill_adjustments.psd") else {
        return;
    };
    let labels: Vec<&str> = adjustments_of(&document)
        .iter()
        .map(|(_, a)| a.label())
        .collect::<Vec<_>>()
        .into_iter()
        .map(|l| match l {
            "Channel Mixer" => "Channel Mixer",
            "Selective Color" => "Selective Color",
            _ => "other",
        })
        .collect();
    assert_eq!(labels.iter().filter(|l| **l == "Channel Mixer").count(), 1);
    assert_eq!(
        labels.iter().filter(|l| **l == "Selective Color").count(),
        1
    );
    let items = document.report.items.join("\n");
    assert!(!items.contains("textbook"), "{items}");
    // Only Gradient Map and Color Lookup remain left out.
    assert!(items.contains("2 adjustment layers"), "{items}");
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
fn the_writer_reproduces_photoshops_neutral_blocks_byte_for_byte() {
    let e = encoded(&Adjustment::ChannelMixer(ChannelMixerParams::identity()));
    assert_eq!(e.key, *b"mixr");
    assert_eq!(e.data, unhex(MIXR_NEUTRAL));
    assert!(e.extra.is_none() && !e.rounding.rounded);
    let e = encoded(&Adjustment::SelectiveColor(SelectiveColorParams::identity()));
    assert_eq!(e.key, *b"selc");
    assert_eq!(e.data, selc_neutral());
}

#[test]
fn the_writer_places_every_value_in_the_layout_the_reader_parses() {
    let a = mixer(
        [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [-200.0, 200.0, -9.0, -100.0],
        ],
        true,
    );
    let e = encoded(&a);
    assert_eq!(
        e.data,
        mixr(
            1,
            1,
            [
                [1, 2, 3, 0, 4],
                [5, 6, 7, 0, 8],
                [-200, 200, -9, 0, -100],
                [0, 0, 0, 100, 0]
            ]
        )
    );
    assert_eq!(e.readback, a);
    let a = selective(
        &[(0, [1.0, 2.0, 3.0, 4.0]), (8, [-5.0, 6.0, -7.0, 8.0])],
        true,
    );
    let e = encoded(&a);
    let mut plates = [[0_i16; 4]; 10];
    plates[1] = [1, 2, 3, 4];
    plates[9] = [-5, 6, -7, 8];
    assert_eq!(e.data, selc(1, 1, plates));
    assert_eq!(e.readback, a);
}

#[test]
fn every_edge_value_reads_back_as_itself() {
    let mut all = Vec::new();
    for v in [-200.0_f32, -199.0, -1.0, 0.0, 1.0, 199.0, 200.0] {
        for mono in [false, true] {
            all.push(mixer([[v, -v, 0.0, 100.0_f32.min(v.abs())]; 3], mono));
        }
    }
    for v in [-100.0_f32, -99.0, -1.0, 0.0, 1.0, 99.0, 100.0] {
        for absolute in [false, true] {
            let all_ranges: Vec<(usize, [f32; 4])> = (0..9).map(|i| (i, [v, -v, v, -v])).collect();
            all.push(selective(&all_ranges, absolute));
        }
    }
    for a in all {
        let e = encoded(&a);
        assert_eq!(e.readback, a, "{a:?}");
        assert!(!e.rounding.rounded, "{a:?}");
        assert_eq!(e.data.len() % 4, 0);
    }
}

#[test]
fn fractional_values_are_rounded_and_itemised() {
    let a = mixer(
        [
            [30.4, 59.0, 11.0, 0.0],
            [0.0, 100.0, 0.0, 0.0],
            [0.0, 0.0, 100.0, 0.0],
        ],
        false,
    );
    let e = encoded(&a);
    assert!(e.rounding.rounded);
    let Adjustment::ChannelMixer(back) = e.readback else {
        unreachable!("mixer")
    };
    assert_eq!(back.row(0), Some([30.0, 59.0, 11.0, 0.0]));
    let a = selective(&[(2, [0.0, 0.0, 0.0, -49.6])], false);
    let e = encoded(&a);
    assert!(e.rounding.rounded);
    let Adjustment::SelectiveColor(back) = e.readback else {
        unreachable!("selective")
    };
    assert_eq!(back.range(2), Some([0.0, 0.0, 0.0, -50.0]));
    // Report lines through the preflight, and none for exact values.
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "M",
        mixer(ChannelMixerParams::identity().rows(), false),
        None,
        0,
    ));
    ok(tree.add_adjustment_layer_at("S", selective(&[(1, [10.0, 0.0, 0.0, 0.0])], true), None, 0));
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    assert!(report.items.is_empty(), "{:?}", report.items);
    ok(tree.add_adjustment_layer_at("M2", a_mixer_fractional(), None, 0));
    ok(tree.add_adjustment_layer_at(
        "S2",
        selective(&[(1, [10.5, 0.0, 0.0, 0.0])], true),
        None,
        0,
    ));
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    let text = report.items.join("\n");
    assert!(
        text.contains(
            "1 Channel Mixer layer's fractional values will be rounded to whole percents"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "1 Selective Color layer's fractional values will be rounded to whole percents"
        ),
        "{text}"
    );
}

fn a_mixer_fractional() -> Adjustment {
    mixer(
        [
            [50.5, 0.0, 0.0, 0.0],
            [0.0, 100.0, 0.0, 0.0],
            [0.0, 0.0, 100.0, 0.0],
        ],
        false,
    )
}

#[test]
fn a_monochrome_mixer_is_itemised_because_its_row_is_unverified() {
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", rect(0, 0, 4, 4), None));
    ok(tree.add_adjustment_layer_at(
        "Mono",
        mixer(
            [
                [30.0, 59.0, 11.0, 0.0],
                [0.0, 100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0, 0.0],
            ],
            true,
        ),
        None,
        0,
    ));
    let report = ok(psd_save_preflight(&tree, (4, 4), PsdSaveOptions::default()));
    let text = report.items.join("\n");
    assert!(
        text.contains(
            "1 Channel Mixer layer's Monochrome Gray mix will be stored in the first output row"
        ),
        "{text}"
    );
}

fn two_doc() -> Doc {
    let mut doc = Doc::new((16, 16));
    doc.layer("Background", rect(0, 0, 16, 16), None);
    let m = ok(doc.tree.add_adjustment_layer_at(
        "Mixer",
        mixer(
            [
                [30.0, 59.0, 11.0, 5.0],
                [-20.0, 120.0, 0.0, 0.0],
                [10.0, 10.0, 80.0, -5.0],
            ],
            false,
        ),
        None,
        0,
    ));
    ok(doc.tree.set_opacity(m, 128.0 / 255.0));
    ok(doc.tree.set_blend_mode(m, aurora_doc::BlendMode::Multiply));
    doc.mask(m, rect(2, 2, 10, 10), (0, 0, 10, 10), |x, y| {
        f32::from(u16::try_from((x * 20 + y) % 255).unwrap_or(0)) / 255.0
    });
    let s = ok(doc.tree.add_adjustment_layer_at(
        "Selective",
        selective(
            &[(0, [10.0, -20.0, 30.0, -40.0]), (7, [5.0, 5.0, 5.0, 5.0])],
            true,
        ),
        None,
        0,
    ));
    ok(doc.tree.set_visible(s, false));
    ok(doc.tree.add_adjustment_layer_at(
        "Mono",
        mixer(
            [
                [30.0, 59.0, 11.0, 0.0],
                [0.0, 100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0, 0.0],
            ],
            true,
        ),
        None,
        0,
    ));
    doc
}

fn id_named(tree: &LayerTree, name: &str) -> LayerId {
    tree.roots()
        .iter()
        .copied()
        .find(|&id| tree.name(id) == Some(name))
        .unwrap_or_else(|| unreachable!("{name}"))
}

#[test]
fn both_kinds_round_trip_through_write_verify_and_read() {
    let mut doc = two_doc();
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("two.psd");
    let outcome = ok(save(&mut doc, &target, false));
    // Only the Monochrome line (and no rounding line): the values are exact.
    assert_eq!(outcome.report.items.len(), 1, "{:?}", outcome.report.items);
    let back = read_back(&ok(std::fs::read(&target)));
    // Both non-neutral kinds are noted as unvalidated on reopen.
    let text = back.report.items.join("\n");
    assert!(
        text.contains("2 Channel Mixer layers are shown with a textbook formula"),
        "{text}"
    );
    assert!(
        text.contains("1 Selective Color layer is shown with a textbook formula"),
        "{text}"
    );
    for name in ["Mixer", "Selective", "Mono"] {
        let (want, got) = (id_named(&doc.tree, name), id_named(&back.layers, name));
        assert_eq!(
            back.layers.adjustment(got),
            doc.tree.adjustment(want),
            "{name}"
        );
        assert_eq!(back.layers.opacity(got), doc.tree.opacity(want), "{name}");
        assert_eq!(
            back.layers.blend_mode(got),
            doc.tree.blend_mode(want),
            "{name}"
        );
        assert_eq!(back.layers.visible(got), doc.tree.visible(want), "{name}");
        assert_eq!(
            back.layers.mask(got).map(|m| m.bounds),
            doc.tree.mask(want).map(|m| m.bounds),
            "{name}"
        );
    }
}

#[test]
fn the_verifier_catches_a_wrong_block_or_readback_for_each_kind() {
    let mut doc = two_doc();
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("two.psd");
    let (bytes, outcome) = ok(doc.write());
    ok(std::fs::write(&path, &bytes));
    ok(super::write::verify_psd_file(&path, &outcome.expected));
    for key in [b"mixr", b"selc"] {
        let Some(i) = outcome
            .expected
            .layers
            .iter()
            .position(|l| l.adjustment.as_ref().is_some_and(|a| a.key == *key))
        else {
            unreachable!("{key:?}")
        };
        let mut wrong = outcome.expected.clone();
        if let Some(a) = wrong.layers.get_mut(i).and_then(|l| l.adjustment.as_mut())
            && let Some(byte) = a.data.get_mut(5)
        {
            *byte ^= 1;
        }
        let err = super::write::verify_psd_file(&path, &wrong);
        assert!(
            matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment blocks")),
            "{key:?}: {err:?}"
        );
        let mut wrong = outcome.expected.clone();
        if let Some(a) = wrong.layers.get_mut(i).and_then(|l| l.adjustment.as_mut()) {
            a.readback = Adjustment::Invert;
        }
        let err = super::write::verify_psd_file(&path, &wrong);
        assert!(
            matches!(&err, Err(IoError::PsdVerifyFailed { what }) if what.contains("adjustment")),
            "{key:?}: {err:?}"
        );
    }
}

/// `adjustments5.psd` / `adjustments5.json` for `scripts/verify_psd_write.py
/// check_adjustments5` (psd-tools).
///
/// ```text
/// AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
///     two_adjustments_fixture_for_psd_tools -- --ignored
/// ```
#[test]
#[ignore = "writes the fixture scripts/verify_psd_write.py reads"]
fn two_adjustments_fixture_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = std::path::Path::new(&out);
    let mut doc = two_doc();
    let (bytes, _) = ok(doc.write());
    ok(std::fs::write(out.join("adjustments5.psd"), &bytes));
    let json = "{\"mixer\": [[30, 59, 11, 5], [-20, 120, 0, 0], [10, 10, 80, -5]], \
        \"mono\": [[30, 59, 11, 0], [0, 100, 0, 0], [0, 0, 100, 0]], \
        \"selective_reds\": [10, -20, 30, -40], \"selective_neutrals\": [5, 5, 5, 5], \
        \"absolute\": true}\n";
    ok(std::fs::write(out.join("adjustments5.json"), json));
}
