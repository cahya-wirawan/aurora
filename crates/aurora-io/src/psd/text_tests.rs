//! Tests for text-layer preservation (task 155): the reader keeps a text
//! layer's `TySh` block and the document's `Txt2` block, the writer writes
//! them back verbatim while they are still true, and drops them (and says
//! so) once the pixels or the position change.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::used_underscore_binding
)] // fixture-building test code over fixed, known-size data

use std::io::Cursor;

use aurora_doc::{History, LayerId, LayerLock, LayerTree, MAX_TEXT_BLOCK_BYTES};
use aurora_tile::TileId;
use half::f16;

use super::smart_tests::{
    Opened, all_layers, aur_bytes, bounds_of, ok, plain_layer, rewrite_entry, store, unicode,
};
use super::test_writer::{TestLayer, TestPsd};
use super::write::{PsdWriteOutcome, verify_psd_file};
use super::{PsdDocument, read};
use crate::error::IoError;

// ---------------------------------------------------------------------
// Fixture bytes
// ---------------------------------------------------------------------

fn utf16_name(name: &str) -> Vec<u8> {
    let mut out = vec![0xFE, 0xFF];
    for u in name.encode_utf16() {
        out.extend_from_slice(&u.to_be_bytes());
    }
    out
}

/// A text engine data stream with a `/FontSet` of `fonts`.
fn engine(fonts: &[&str]) -> Vec<u8> {
    let mut e = b"\n<<\n\t/EngineDict\n\t<<\n\t\t/Editor\n\t\t<<\n\t\t\t/Text (\xfe\xff\x00h\x00i)\n\t\t>>\n\t>>\n\t/ResourceDict\n\t<<\n\t\t/FontSet [\n".to_vec();
    for f in fonts {
        e.extend_from_slice(b"\t\t<<\n\t\t\t/Name (");
        e.extend_from_slice(&utf16_name(f));
        e.extend_from_slice(b")\n\t\t\t/Script 0\n\t\t>>\n");
    }
    e.extend_from_slice(b"\t\t]\n\t>>\n>>");
    e
}

/// A `TySh` block: version 1, a transform, text version 50, descriptor
/// version 16, a descriptor-ish body holding `EngineData`, a trailing box;
/// a multiple of four bytes long like Photoshop's.
fn tysh(fonts: &[&str], seed: u8) -> Vec<u8> {
    let mut b = vec![0, 1];
    for i in 0..6u8 {
        b.extend_from_slice(&f64::from(seed + i).to_be_bytes());
    }
    b.extend_from_slice(&[0, 50, 0, 0, 0, 16]);
    b.extend_from_slice(&unicode("")); // descriptor name
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(b"TxLr");
    b.extend_from_slice(&1u32.to_be_bytes());
    b.extend_from_slice(&10u32.to_be_bytes());
    b.extend_from_slice(b"EngineData");
    b.extend_from_slice(b"tdta");
    let e = engine(fonts);
    b.extend_from_slice(&(e.len() as u32).to_be_bytes());
    b.extend_from_slice(&e);
    b.extend_from_slice(&[0; 32]);
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    b
}

/// A document-level engine block (deliberately not a multiple of four).
fn txt2() -> Vec<u8> {
    let mut t = b" /DocumentResources << /FontSet << /Resources [ ] >> >>".to_vec();
    while t.len().is_multiple_of(4) {
        t.push(b' ');
    }
    t.push(b'!');
    t
}

fn rgba(width: i32, height: i32, seed: u16) -> Vec<[u16; 4]> {
    (0..width * height)
        .map(|i| {
            let i = i as u16;
            [
                (i * 7 + seed) % 256,
                (i * 13 + seed) % 256,
                (i * 29 + seed) % 256,
                if i.is_multiple_of(5) { 0 } else { 255 },
            ]
        })
        .collect()
}

fn text_layer(name: &str, left: i32, top: i32, seed: u16, fonts: &[&str]) -> TestLayer {
    TestLayer::pixels(name, left, top, 4, 4, 8, &rgba(4, 4, seed)).with(|l| {
        l.blocks = vec![(*b"TySh", tysh(fonts, seed as u8))];
    })
}

/// Eight by eight, bottom to top: a plain layer, two text layers, a plain
/// one; a `Txt2` block.
fn text_psd() -> TestPsd {
    TestPsd::new(1, 8, 8, 8).with(|p| {
        p.layers = vec![
            plain_layer("Background"),
            text_layer("Title", 1, 1, 10, &["ArialMT", "Myriad Pro-Regular"]),
            text_layer("Body", 3, 2, 50, &["ArialMT"]),
            plain_layer("Top"),
        ];
        p.globals = vec![(*b"Txt2", txt2())];
    })
}

fn text_names(tree: &LayerTree) -> Vec<String> {
    all_layers(tree)
        .into_iter()
        .filter(|id| tree.text_layer(*id).is_some())
        .filter_map(|id| tree.name(id).map(str::to_owned))
        .collect()
}

fn report_text(outcome: &PsdWriteOutcome) -> String {
    outcome.report.items.join("\n")
}

fn paint(opened: &mut Opened, name: &str) {
    let surface = opened.surface(name);
    let tile = ok(opened.store.get_mut(surface, TileId { x: 0, y: 0 }));
    tile.texels_mut()[3] = f16::from_f32(0.125);
}

// ---------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------

#[test]
fn the_reader_keeps_text_blocks_verbatim_with_the_engine_block() {
    let opened = Opened::new(&text_psd().write());
    let tree = &opened.document.layers;
    assert_eq!(text_names(tree), vec!["Body", "Title"], "paint order");
    let title = opened.layer("Title");
    let Some(t) = tree.text_layer(title) else {
        unreachable!("kept");
    };
    assert_eq!(t.key, *b"TySh");
    assert_eq!(t.data, tysh(&["ArialMT", "Myriad Pro-Regular"], 10));
    assert_eq!(Some(t.bounds), tree.bounds(title));
    assert_eq!(
        tree.text_engine_data().map(|e| e.to_vec()),
        Some(txt2()),
        "exact length, no padding"
    );
    assert!(tree.text_layer(opened.layer("Top")).is_none());
}

#[test]
fn the_open_time_fingerprint_is_the_one_the_store_computes() {
    let mut opened = Opened::new(&text_psd().write());
    for name in ["Title", "Body"] {
        let id = opened.layer(name);
        let surface = opened.surface(name);
        let bounds = bounds_of(&opened.document.layers, id);
        let want = opened
            .document
            .layers
            .text_layer(id)
            .map(|t| t.pixel_fingerprint);
        let got = ok(aurora_doc::smart_object::surface_fingerprint(
            &mut opened.store,
            surface,
            bounds,
        ));
        assert_eq!(want, Some(got), "{name}");
        assert_ne!(got, 0, "{name} has pixels");
    }
}

#[test]
fn the_report_says_kept_for_photoshop_and_names_the_fonts() {
    let opened = Opened::new(&text_psd().write());
    let items = &opened.document.report.items;
    assert_eq!(items.len(), 1, "{items:?}");
    let line = &items[0];
    assert!(line.starts_with("2 text layers opened as pixels"), "{line}");
    assert!(line.contains("can't be edited in Aurora"), "{line}");
    assert!(line.contains("kept"), "{line}");
    assert!(line.contains("hasn't checked this in Photoshop"), "{line}");
    assert!(
        !items.iter().any(|i| i.contains("colour profile")),
        "no profile"
    );
    assert!(
        line.contains("Fonts they use: ArialMT, Myriad Pro-Regular."),
        "{line}"
    );
}

#[test]
fn a_layer_whose_text_data_is_not_kept_is_reported_as_not_kept() {
    let mut psd = text_psd();
    // Over the per-block bound; too short to be a TySh; no pixels.
    let mut big = tysh(&["A"], 1);
    big.resize(MAX_TEXT_BLOCK_BYTES + 4, 0);
    psd.layers[1].blocks = vec![(*b"TySh", big)];
    psd.layers[2].blocks = vec![(*b"TySh", vec![0, 1, 2, 3])];
    psd.layers.push(TestLayer::empty("Empty text").with(|l| {
        l.blocks = vec![(*b"TySh", tysh(&["A"], 2))];
    }));
    let opened = Opened::new(&psd.write());
    assert!(text_names(&opened.document.layers).is_empty());
    assert!(opened.document.layers.text_engine_data().is_none());
    let items = &opened.document.report.items;
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(
        items[0].starts_with("3 text layers opened as pixels"),
        "{items:?}"
    );
    assert!(items[0].contains("isn't kept"), "{items:?}");
}

#[test]
fn a_mix_of_kept_and_unkept_text_layers_gets_both_lines() {
    let mut psd = text_psd();
    psd.layers[2].blocks = vec![(*b"TySh", vec![0, 1, 2, 3])];
    let opened = Opened::new(&psd.write());
    assert_eq!(text_names(&opened.document.layers), vec!["Title"]);
    let items = &opened.document.report.items;
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(items.iter().any(|i| i.starts_with("1 text layer opened")));
    assert!(items.iter().any(|i| i.contains("isn't kept")));
}

#[test]
fn a_grayscale_file_does_not_keep_text() {
    // Photoshop's text colours would be grey; the saved file is RGB.
    let psd = TestPsd::new(1, 8, 8, 8).with(|p| {
        p.color_mode = 1;
        p.channels = 1;
        p.layers = vec![
            TestLayer::gray("T", 0, 0, 4, 4, 8, &[[10, 255]; 16]).with(|l| {
                l.blocks = vec![(*b"TySh", tysh(&["A"], 3))];
            }),
        ];
        p.globals = vec![(*b"Txt2", txt2())];
    });
    let opened = Opened::new(&psd.write());
    assert!(text_names(&opened.document.layers).is_empty());
    assert!(opened.document.layers.text_engine_data().is_none());
}

#[test]
fn the_total_bound_keeps_the_first_layers_and_drops_the_rest() {
    let per = MAX_TEXT_BLOCK_BYTES;
    let count = aurora_doc::MAX_TEXT_TOTAL_BYTES / per;
    let mut psd = TestPsd::new(1, 8, 8, 8);
    psd.layers = (0..=count)
        .map(|i| {
            let mut block = tysh(&["A"], i as u8);
            block.resize(per, 0);
            TestLayer::pixels(&format!("T{i}"), 0, 0, 4, 4, 8, &rgba(4, 4, i as u16)).with(|l| {
                l.blocks = vec![(*b"TySh", block.clone())];
            })
        })
        .collect();
    let opened = Opened::new(&psd.write());
    let kept = text_names(&opened.document.layers);
    assert_eq!(kept.len(), count, "{kept:?}");
    assert!(!kept.contains(&format!("T{count}")), "the last one is over");
    assert!(
        opened
            .document
            .report
            .items
            .iter()
            .any(|i| i.contains("isn't kept"))
    );
}

#[test]
fn many_large_blocks_never_make_the_reader_copy_more_than_the_bound() {
    // 40 records of 1 MiB each (40 MiB of file): the reader must stop
    // copying once the file-wide bound is spent, at the point each record
    // is read, not afterwards.
    let per = MAX_TEXT_BLOCK_BYTES;
    let mut psd = TestPsd::new(1, 8, 8, 8);
    psd.layers = (0..40)
        .map(|i| {
            let mut block = tysh(&["A"], i as u8);
            block.resize(per, 0);
            TestLayer::pixels(&format!("T{i}"), 0, 0, 4, 4, 8, &rgba(4, 4, i as u16)).with(|l| {
                l.blocks = vec![(*b"TySh", block.clone())];
            })
        })
        .collect();
    let bytes = psd.write();
    let (_, records) = super::tests::records_of(&bytes);
    assert_eq!(records.len(), 40);
    let copied: usize = records
        .iter()
        .filter_map(|r| r.text_candidate.as_ref())
        .map(|c| c.data.len())
        .sum();
    assert_eq!(
        copied,
        aurora_doc::MAX_TEXT_TOTAL_BYTES,
        "exactly the bound"
    );
    let with: Vec<bool> = records.iter().map(|r| r.text_candidate.is_some()).collect();
    assert_eq!(
        with.iter().filter(|b| **b).count(),
        aurora_doc::MAX_TEXT_TOTAL_BYTES / per
    );
    assert!(with[..8].iter().all(|b| *b), "the first ones");
    assert!(with[8..].iter().all(|b| !*b), "none after the bound");
}

#[test]
fn a_32_bit_file_does_not_keep_text() {
    // Its pixels were converted from linear light; the text colours were
    // not, so the kept data would not match.
    let psd = TestPsd::new(1, 8, 8, 32).with(|p| {
        p.layers = vec![
            TestLayer::floats("T", 0, 0, 4, 4, &[[0.5, 0.25, 0.75, 1.0]; 16]).with(|l| {
                l.blocks = vec![(*b"TySh", tysh(&["A"], 3))];
            }),
        ];
        p.globals = vec![(*b"Txt2", txt2())];
    });
    let opened = Opened::new(&psd.write());
    assert!(text_names(&opened.document.layers).is_empty());
    assert!(opened.document.layers.text_engine_data().is_none());
    let items = &opened.document.report.items;
    assert!(
        items
            .iter()
            .any(|i| i.contains("text layer") && i.contains("isn't kept")),
        "{items:?}"
    );
}

#[test]
fn an_oversized_or_missing_engine_block_keeps_the_layers_without_it() {
    let huge = vec![b'x'; aurora_doc::MAX_TEXT_ENGINE_BYTES + 1];
    for globals in [vec![(*b"Txt2", huge)], Vec::new()] {
        let psd = text_psd().with(|p| p.globals = globals.clone());
        let opened = Opened::new(&psd.write());
        assert_eq!(text_names(&opened.document.layers).len(), 2);
        assert!(opened.document.layers.text_engine_data().is_none());
    }
    // The bound itself is kept.
    let psd = text_psd().with(|p| {
        p.globals = vec![(*b"Txt2", vec![b'x'; aurora_doc::MAX_TEXT_ENGINE_BYTES])];
    });
    let opened = Opened::new(&psd.write());
    assert_eq!(
        opened.document.layers.text_engine_data().map(|e| e.len()),
        Some(aurora_doc::MAX_TEXT_ENGINE_BYTES)
    );
}

#[test]
fn the_legacy_key_is_kept_too() {
    let mut psd = text_psd();
    psd.layers[2].blocks = vec![(*b"tySh", tysh(&["A"], 4))];
    let opened = Opened::new(&psd.write());
    let body = opened.layer("Body");
    assert_eq!(
        opened.document.layers.text_layer(body).map(|t| t.key),
        Some(*b"tySh")
    );
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

fn dropped(opened: &mut Opened) -> (PsdWriteOutcome, LayerTree, Vec<u8>) {
    let (bytes, outcome) = opened.write();
    let again = ok(read(&bytes));
    (outcome, again.layers, bytes)
}

#[test]
fn unchanged_text_layers_round_trip_through_a_save() {
    let mut opened = Opened::new(&text_psd().write());
    let (bytes, outcome) = opened.write();
    assert!(
        outcome.report.is_empty(),
        "lossless: {:?}",
        outcome.report.items
    );
    assert_eq!(
        outcome.expected.text_engine.map(|(len, _)| len),
        Some(txt2().len() as u64)
    );
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("out.psd");
    ok(std::fs::write(&path, &bytes));
    ok(verify_psd_file(&path, &outcome.expected));

    let mut again = Opened::new(&bytes);
    assert_eq!(
        text_names(&again.document.layers),
        text_names(&opened.document.layers)
    );
    for name in ["Title", "Body"] {
        let (Some(a), Some(b)) = (
            opened.document.layers.text_layer(opened.layer(name)),
            again.document.layers.text_layer(again.layer(name)),
        ) else {
            unreachable!("{name} kept");
        };
        // The block is a multiple of four bytes, so byte for byte.
        assert_eq!(a.data, b.data, "{name}");
        assert_eq!(a.key, b.key, "{name}");
    }
    assert_eq!(
        again.document.layers.text_engine_data().map(|e| e.to_vec()),
        Some(txt2())
    );
    // The data is itself still true against the reopened pixels, so a
    // second save is lossless again (stable).
    let (_, second) = again.write();
    assert!(second.report.is_empty(), "{:?}", second.report.items);
    assert!(second.expected.text_engine.is_some());
}

#[test]
fn painting_a_text_layer_drops_its_text_and_says_so() {
    let mut opened = Opened::new(&text_psd().write());
    paint(&mut opened, "Title");
    let (outcome, tree, _) = dropped(&mut opened);
    let text = report_text(&outcome);
    assert!(text.contains("1 text layer will be saved as ordinary pixel layers"));
    assert!(text.contains("painted or erased"), "{text}");
    assert!(text.contains("\u{201c}Title\u{201d}"), "{text}");
    assert!(!text.contains("Body"), "only the edited layer: {text}");
    assert_eq!(text_names(&tree), vec!["Body"]);
    // The engine block still goes with the layer that kept its text.
    assert!(tree.text_engine_data().is_some());
}

#[test]
fn with_every_text_layer_dropped_no_engine_block_is_written() {
    let mut opened = Opened::new(&text_psd().write());
    paint(&mut opened, "Title");
    paint(&mut opened, "Body");
    let (outcome, tree, bytes) = dropped(&mut opened);
    assert!(report_text(&outcome).contains("2 text layers will be saved"));
    assert!(text_names(&tree).is_empty());
    assert!(tree.text_engine_data().is_none());
    assert_eq!(outcome.expected.text_engine, None);
    assert!(!bytes.windows(4).any(|w| w == b"Txt2"));
    assert!(!bytes.windows(4).any(|w| w == b"TySh"));
}

#[test]
fn erasing_and_putting_the_pixels_back_keeps_the_text() {
    let mut opened = Opened::new(&text_psd().write());
    let surface = opened.surface("Body");
    let id = TileId { x: 0, y: 0 };
    let original = {
        let tile = ok(opened.store.get_mut(surface, id));
        let original = tile.texels().to_vec();
        tile.texels_mut().fill(f16::ZERO);
        original
    };
    let (_, outcome) = opened.write();
    assert!(report_text(&outcome).contains("Body"), "erased");
    ok(opened.store.get_mut(surface, id))
        .texels_mut()
        .copy_from_slice(&original);
    let (outcome, tree, _) = dropped(&mut opened);
    assert!(outcome.report.is_empty(), "{:?}", outcome.report.items);
    assert_eq!(text_names(&tree).len(), 2, "an undone edit keeps it");
}

#[test]
fn moving_a_text_layer_drops_its_text() {
    let mut opened = Opened::new(&text_psd().write());
    let id = opened.layer("Body");
    let mut bounds = bounds_of(&opened.document.layers, id);
    bounds.x += 1;
    ok(opened.document.layers.set_bounds(id, bounds));
    let (outcome, tree, _) = dropped(&mut opened);
    let text = report_text(&outcome);
    assert!(text.contains("they were moved"), "{text}");
    assert!(text.contains("Body"), "{text}");
    assert_eq!(text_names(&tree), vec!["Title"]);
}

#[test]
fn opacity_blend_visibility_locks_and_masks_keep_the_text() {
    let mut opened = Opened::new(&text_psd().write());
    let id = opened.layer("Title");
    let tree = &mut opened.document.layers;
    ok(tree.set_opacity(id, 0.5));
    ok(tree.set_fill_opacity(id, 0.25));
    ok(tree.set_blend_mode(id, aurora_doc::BlendMode::Multiply));
    ok(tree.set_visible(id, false));
    ok(tree.set_lock(id, LayerLock::all()));
    let bounds = bounds_of(tree, id);
    ok(tree.add_mask(id, bounds));
    let (outcome, tree, _) = dropped(&mut opened);
    assert_eq!(text_names(&tree).len(), 2);
    assert!(!report_text(&outcome).contains("text layer"));
}

#[test]
fn a_layer_with_no_pixels_never_keeps_text_even_with_a_matching_fingerprint() {
    // Nothing renders the text: an empty layer's fingerprint is 0.
    let mut opened = Opened::new(&text_psd().write());
    let mut history = History::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    };
    let id = ok(history.add_pixel_layer(&mut opened.document.layers, "Blank", bounds, None));
    ok(opened.document.layers.set_text_layer(
        id,
        Some(aurora_doc::TextLayer {
            key: *b"TySh",
            data: tysh(&["A"], 5),
            bounds,
            pixel_fingerprint: 0,
        }),
    ));
    let (outcome, tree, _) = dropped(&mut opened);
    let text = report_text(&outcome);
    assert!(text.contains("Blank"), "{outcome:?}");
    assert!(
        text.contains("hold no stored pixels"),
        "an accurate reason: {text}"
    );
    assert!(!text.contains("painted or erased"), "{text}");
    assert_eq!(text_names(&tree).len(), 2);
}

// ---------------------------------------------------------------------
// .aur
// ---------------------------------------------------------------------

fn reopened_from_aur(bytes: &[u8]) -> Opened {
    let (_dir, mut store2) = store();
    let doc = ok(crate::aur::read(Cursor::new(bytes), &mut store2));
    Opened {
        _dir,
        store: store2,
        document: PsdDocument {
            layers: doc.layers,
            history: doc.history,
            canvas_size: doc.canvas_size,
            pixels: Vec::new(),
            masks: Vec::new(),
            report: super::PsdImportReport::default(),
            colour: crate::ColourImport::default(),
        },
    }
}

#[test]
fn text_layers_survive_an_aur_round_trip_and_a_later_psd_save() {
    let mut opened = Opened::new(&text_psd().write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let mut again = reopened_from_aur(&bytes);
    assert_eq!(
        text_names(&again.document.layers),
        text_names(&opened.document.layers)
    );
    for name in ["Title", "Body"] {
        assert_eq!(
            again.document.layers.text_layer(again.layer(name)),
            opened.document.layers.text_layer(opened.layer(name)),
            "{name}"
        );
    }
    assert_eq!(
        again.document.layers.text_engine_data(),
        opened.document.layers.text_engine_data()
    );
    let (_, outcome) = again.write();
    assert!(outcome.report.is_empty(), "{:?}", outcome.report.items);
    assert!(outcome.expected.text_engine.is_some());
}

#[test]
fn the_journal_carries_text_data_through_a_delete_and_undo() {
    let mut opened = Opened::new(&text_psd().write());
    // The open's own history holds the SetTextLayer ops; delete + undo adds
    // a Restore carrier on top.
    let mut history = std::mem::replace(&mut opened.document.history, History::new());
    let id = opened.layer("Title");
    ok(history.remove(&mut opened.document.layers, id));
    ok(history.undo(&mut opened.document.layers));
    let bytes = aur_bytes(&mut opened, &history);
    let again = reopened_from_aur(&bytes);
    assert_eq!(text_names(&again.document.layers).len(), 2);
    // Replaying the journal alone reaches the same text data.
    let replayed = ok(History::load_journal(&ok(history.save_journal())));
    let tree = ok(replayed.replay());
    assert_eq!(text_names(&tree).len(), 2);
}

#[test]
fn a_document_without_text_writes_no_text_entry() {
    let psd = TestPsd::new(1, 8, 8, 8).with(|p| p.layers = vec![plain_layer("Only")]);
    let mut opened = Opened::new(&psd.write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let archive = ok(zip::ZipArchive::new(Cursor::new(&bytes)));
    assert!(
        archive
            .file_names()
            .filter_map(Result::ok)
            .all(|n| n != "text-layers")
    );
}

fn entry_of(bytes: &[u8], name: &str) -> Vec<u8> {
    use std::io::Read;
    let mut archive = ok(zip::ZipArchive::new(Cursor::new(bytes)));
    let mut out = Vec::new();
    ok(ok(archive.by_name(name)).read_to_end(&mut out));
    out
}

#[test]
fn damaged_newer_or_older_text_entries_refuse_the_open() {
    let mut opened = Opened::new(&text_psd().write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let open = |b: &[u8]| {
        let (_dir, mut s) = store();
        crate::aur::read(Cursor::new(b), &mut s).map(|_| ())
    };
    assert!(open(&bytes).is_ok());
    let meta = entry_of(&bytes, "text-layers");
    // A newer version.
    let mut newer = meta.clone();
    newer[0] = 2;
    match open(&rewrite_entry(&bytes, "text-layers", Some(&newer))) {
        Err(IoError::ManifestDeserialization(msg)) => {
            assert!(msg.contains("newer Aurora"), "{msg}");
        }
        other => unreachable!("{other:?}"),
    }
    // An older (unknown) one.
    let mut older = meta.clone();
    older[0] = 0;
    match open(&rewrite_entry(&bytes, "text-layers", Some(&older))) {
        Err(IoError::ManifestDeserialization(msg)) => {
            assert!(msg.contains("damaged or of an unknown kind"), "{msg}");
        }
        other => unreachable!("{other:?}"),
    }
    // Truncated bytes.
    assert!(open(&rewrite_entry(&bytes, "text-layers", Some(&meta[..3]))).is_err());
    // No entry at all: opens, with no text data.
    let (_dir, mut s) = store();
    let doc = ok(crate::aur::read(
        Cursor::new(rewrite_entry(&bytes, "text-layers", None)),
        &mut s,
    ));
    assert!(text_names(&doc.layers).is_empty());
    assert!(doc.layers.text_engine_data().is_none());
}

#[test]
fn hostile_text_entries_are_refused() {
    let mut opened = Opened::new(&text_psd().write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let title = opened.layer("Title").to_raw();
    let open = |payload: &[u8]| {
        let (_dir, mut s) = store();
        crate::aur::read(
            Cursor::new(rewrite_entry(&bytes, "text-layers", Some(payload))),
            &mut s,
        )
        .map(|_| ())
    };
    let layer = |key: [u8; 4], data: Vec<u8>| aurora_doc::TextLayer {
        key,
        data,
        bounds: aurora_core::Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        },
        pixel_fingerprint: 0,
    };
    let wire = |layers: Vec<(u64, aurora_doc::TextLayer)>, engine: Option<Vec<u8>>| {
        ok(postcard::to_allocvec(&(1u32, layers, engine)))
    };
    // Well-formed: accepted.
    assert!(
        open(&wire(
            vec![(title, layer(*b"TySh", vec![1; 8]))],
            Some(vec![1; 8])
        ))
        .is_ok()
    );
    // A layer that does not exist; a wrong key; an empty or oversized
    // block; an oversized or empty engine block.
    assert!(open(&wire(vec![(9999, layer(*b"TySh", vec![1; 8]))], None)).is_err());
    assert!(open(&wire(vec![(title, layer(*b"SoLd", vec![1; 8]))], None)).is_err());
    assert!(open(&wire(vec![(title, layer(*b"TySh", Vec::new()))], None)).is_err());
    assert!(
        open(&wire(
            vec![(title, layer(*b"TySh", vec![1; MAX_TEXT_BLOCK_BYTES + 1]))],
            None
        ))
        .is_err()
    );
    assert!(
        open(&wire(
            Vec::new(),
            Some(vec![0; aurora_doc::MAX_TEXT_ENGINE_BYTES + 1])
        ))
        .is_err()
    );
    assert!(open(&wire(Vec::new(), Some(Vec::new()))).is_err());
}

// ---------------------------------------------------------------------
// Real Photoshop files
// ---------------------------------------------------------------------

fn corpus(path: &str) -> Option<Vec<u8>> {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
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

/// One layer's texels over `x0..x1, y0..y1` in document space.
fn doc_pixels(
    opened: &mut Opened,
    id: LayerId,
    (x0, y0, x1, y1): (i64, i64, i64, i64),
) -> Vec<u16> {
    let bounds = bounds_of(&opened.document.layers, id);
    let Some(surface) = opened.document.layers.surface_id(id) else {
        unreachable!("a pixel layer");
    };
    let mut out = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let (lx, ly) = (x - bounds.x, y - bounds.y);
            let inside =
                lx >= 0 && ly >= 0 && lx < i64::from(bounds.width) && ly < i64::from(bounds.height);
            let tile_id = TileId {
                x: (lx.max(0) as u32) / aurora_tile::TILE,
                y: (ly.max(0) as u32) / aurora_tile::TILE,
            };
            if !inside || !opened.store.contains_tile(surface, tile_id) {
                out.extend_from_slice(&[0; 4]);
                continue;
            }
            let tile = ok(opened.store.get(surface, tile_id));
            let at = (((ly as u32 % aurora_tile::TILE) * aurora_tile::TILE
                + (lx as u32 % aurora_tile::TILE))
                * 4) as usize;
            out.extend(tile.texels()[at..at + 4].iter().map(|v| v.to_bits()));
        }
    }
    out
}

/// Opens a real file, saves it, verifies the written file with Aurora's own
/// two passes, reopens it and checks the text blocks are byte-identical and
/// the text layers' pixels unchanged. Returns the number of text layers.
fn round_trip_real(path: &str, compare_pixels: bool) -> Option<usize> {
    let bytes = corpus(path)?;
    let mut opened = Opened::new(&bytes);
    let count = text_names(&opened.document.layers).len();
    assert!(count > 0, "{path} has text layers");
    assert!(
        opened.document.layers.text_engine_data().is_some(),
        "{path}"
    );
    let (out_bytes, outcome) = opened.write();
    let text_lines: Vec<&String> = outcome
        .report
        .items
        .iter()
        .filter(|i| i.contains("text layer"))
        .collect();
    assert!(text_lines.is_empty(), "{path}: {text_lines:?}");
    let dir = ok(tempfile::tempdir());
    let out = dir.path().join("out.psd");
    ok(std::fs::write(&out, &out_bytes));
    ok(verify_psd_file(&out, &outcome.expected));
    let mut again = Opened::new(&out_bytes);
    assert_eq!(text_names(&again.document.layers).len(), count, "{path}");
    // The blocks: same order, byte for byte (Photoshop's are multiples of
    // four bytes long, so no padding was added).
    let blocks = |tree: &LayerTree| -> Vec<Vec<u8>> {
        all_layers(tree)
            .into_iter()
            .filter_map(|id| tree.text_layer(id).map(|t| t.data.clone()))
            .collect()
    };
    assert_eq!(
        blocks(&again.document.layers),
        blocks(&opened.document.layers),
        "{path}: TySh blocks byte-identical"
    );
    assert_eq!(
        again.document.layers.text_engine_data().map(|e| e.to_vec()),
        opened
            .document
            .layers
            .text_engine_data()
            .map(|e| e.to_vec()),
        "{path}: Txt2 byte-identical"
    );
    // The written file holds each block verbatim.
    for block in blocks(&opened.document.layers) {
        assert!(
            out_bytes
                .windows(block.len())
                .any(|w| w == block.as_slice()),
            "{path}"
        );
    }
    if compare_pixels {
        let canvas = (
            0,
            0,
            i64::from(opened.document.canvas_size.0),
            i64::from(opened.document.canvas_size.1),
        );
        for name in text_names(&opened.document.layers) {
            let (a, b) = (opened.layer(&name), again.layer(&name));
            assert_eq!(
                doc_pixels(&mut opened, a, canvas),
                doc_pixels(&mut again, b, canvas),
                "{path}: {name} pixels unchanged"
            );
        }
    }
    // The reopened data is itself still true.
    let (_, second) = again.write();
    assert!(
        second
            .report
            .items
            .iter()
            .all(|i| !i.contains("text layer")),
        "{path}: {:?}",
        second.report.items
    );
    Some(count)
}

#[test]
fn photoshops_own_text_files_round_trip() {
    let base = "corpora/psd/reference/psd-tools-fixtures";
    for (file, layers) in [
        ("text.psd", 1),
        ("layers/type-layer.psd", 1),
        ("layers-minimal/type-layer.psd", 1),
    ] {
        if let Some(count) = round_trip_real(&format!("{base}/{file}"), true) {
            assert_eq!(count, layers, "{file}");
        }
    }
}

#[test]
fn photoshops_32_bit_text_file_is_not_kept() {
    // `pen-text.psd` is a 32-bit file: its pixels were converted from linear
    // light, so its text data is not kept.
    let Some(bytes) = corpus("corpora/psd/reference/psd-tools-fixtures/pen-text.psd") else {
        return;
    };
    let opened = Opened::new(&bytes);
    assert!(text_names(&opened.document.layers).is_empty());
    assert!(opened.document.layers.text_engine_data().is_none());
    assert!(
        opened
            .document
            .report
            .items
            .iter()
            .any(|i| i.starts_with("2 text layers opened as pixels") && i.contains("isn't kept")),
        "{:?}",
        opened.document.report.items
    );
}

#[test]
fn photoshops_text_layers_with_effects_round_trip() {
    let base = "corpora/psd/reference/psd-tools-fixtures";
    if let Some(count) = round_trip_real(&format!("{base}/effect-stroke-gradient.psd"), false) {
        assert_eq!(count, 2);
    }
}

#[test]
fn the_nyepi_template_round_trips_its_text_layers() {
    let path = "images/flat-design-nyepi-template/6739968.psd";
    let Some(bytes) = corpus(path) else {
        return;
    };
    let opened = Opened::new(&bytes);
    let count = text_names(&opened.document.layers).len();
    println!(
        "{count} text layers, report: {:?}",
        opened.document.report.items
    );
    assert!(count >= 30);
    assert!(
        opened
            .document
            .report
            .items
            .iter()
            .any(|i| i.contains("original colour profile")),
        "Adobe RGB file: the colour caveat"
    );
    drop(opened);
    let kept = round_trip_real(path, false);
    assert_eq!(kept, Some(count));
}

/// Writes the files `scripts/verify_psd_write.py check_text` checks with
/// psd-tools (`AURORA_PSD_FIXTURE_DIR`): a real Photoshop file opened and
/// saved, with a JSON of the expected text and fonts.
#[test]
#[ignore = "writes fixtures for scripts/verify_psd_write.py"]
fn write_text_fixtures_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = std::path::Path::new(&out);
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    let mut index = Vec::new();
    for (file, tag) in [
        ("text.psd", "text"),
        ("layers/type-layer.psd", "type-layer"),
    ] {
        let source = base.join(file);
        let mut opened = Opened::new(&ok(std::fs::read(&source)));
        let (bytes, _) = opened.write();
        ok(std::fs::write(
            out.join(format!("{tag}-roundtrip.psd")),
            bytes,
        ));
        index.push(format!(
            "{{\"tag\":{tag:?},\"original\":{:?}}}",
            source.display().to_string()
        ));
    }
    // A synthetic document with one text layer dropped by an edit.
    let mut opened = Opened::new(&text_psd().write());
    paint(&mut opened, "Title");
    let (bytes, _) = opened.write();
    ok(std::fs::write(out.join("text-dropped.psd"), bytes));
    ok(std::fs::write(
        out.join("text.json"),
        format!("{{\"files\":[{}]}}", index.join(",")),
    ));
}
