//! Tests for smart-object preservation (0.200.0, round S1): the reader
//! keeps a smart object's blocks and embedded file, the writer writes them
//! back verbatim while they are still true, and drops them (and says so)
//! once the pixels or the placement change.

#![allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::used_underscore_binding,
    clippy::format_collect,
    clippy::explicit_auto_deref,
    clippy::needless_borrows_for_generic_args,
    clippy::manual_is_multiple_of
)] // fixture-building test code over fixed, known-size data

use std::io::Cursor;
use std::num::NonZeroUsize;

use aurora_color::IccProfile;
use aurora_core::Rect;
use aurora_doc::{History, LayerId, LayerLock, LayerTree};
use aurora_tile::{TileId, TileStore};
use half::f16;

use super::test_writer::{TestLayer, TestPsd};
use super::write::{PsdSaveOptions, PsdWriteOutcome, verify_psd_file, write_psd};
use super::{PsdDocument, read};
use crate::error::IoError;
use crate::image::Image;

const ID_A: &str = "20031581-1068-5645-bb2e-5dc035354e3b";
const ID_B: &str = "b711f658-fef8-b041-b0ad-cb28b01a33ac";
const UNUSED: &str = "00000000-0000-0000-0000-000000000000";

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = NonZeroUsize::new(64) else {
        unreachable!("64 is non-zero");
    };
    let store = ok(TileStore::new(dir.path().to_path_buf(), budget));
    (dir, store)
}

// ---------------------------------------------------------------------
// Fixture bytes
// ---------------------------------------------------------------------

fn unicode(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = (units.len() as u32).to_be_bytes().to_vec();
    for unit in units {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}

/// `SoLd`: `soLD`, version 4, descriptor version 16, a descriptor of one
/// `Idnt` text item.
fn sold(id: &str) -> Vec<u8> {
    let mut data = b"soLD".to_vec();
    data.extend_from_slice(&4u32.to_be_bytes());
    data.extend_from_slice(&16u32.to_be_bytes());
    data.extend_from_slice(&unicode("")); // name
    data.extend_from_slice(&0u32.to_be_bytes()); // class id: a 4-byte code
    data.extend_from_slice(b"null");
    data.extend_from_slice(&1u32.to_be_bytes()); // one item
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(b"Idnt");
    data.extend_from_slice(b"TEXT");
    data.extend_from_slice(&unicode(id));
    // Placement data this fixture does not interpret, padded to four.
    data.extend_from_slice(&[7, 7, 7, 7]);
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    data
}

/// `PlLd`: `plcL`, version 3, the ID as a Pascal string, filler.
fn plld(id: &str) -> Vec<u8> {
    let mut data = b"plcL".to_vec();
    data.extend_from_slice(&3u32.to_be_bytes());
    data.push(id.len() as u8);
    data.extend_from_slice(id.as_bytes());
    data.extend_from_slice(&[1, 2, 3]);
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    data
}

/// A framed linked-layer entry (`liFD`, version 7) holding `payload`.
fn linked_entry(id: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = b"liFD".to_vec();
    body.extend_from_slice(&7u32.to_be_bytes());
    body.push(id.len() as u8);
    body.extend_from_slice(id.as_bytes());
    body.extend_from_slice(&unicode("Vector Smart Object.ai"));
    body.extend_from_slice(b"PDF ART5");
    body.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    body.push(0); // no open-file descriptor
    body.extend_from_slice(payload);
    body.extend_from_slice(&unicode("uuid:1234"));
    body.extend_from_slice(&0f64.to_be_bytes());
    body.push(0);
    let mut framed = (body.len() as u64).to_be_bytes().to_vec();
    framed.extend_from_slice(&body);
    while !framed.len().is_multiple_of(4) {
        framed.push(0);
    }
    framed
}

fn payload(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn rgba(width: i32, height: i32, seed: u16) -> Vec<[u16; 4]> {
    (0..width * height)
        .map(|i| {
            let i = i as u16;
            [
                (i * 7 + seed) % 256,
                (i * 13 + seed) % 256,
                (i * 29 + seed) % 256,
                if i % 5 == 0 { 0 } else { 255 },
            ]
        })
        .collect()
}

fn smart_layer(name: &str, id: &str, left: i32, top: i32, seed: u16) -> TestLayer {
    TestLayer::pixels(name, left, top, 4, 4, 8, &rgba(4, 4, seed)).with(|l| {
        l.blocks = vec![
            (*b"SoLd", sold(id)),
            (*b"PlLd", plld(id)),
            (*b"fxrp", vec![0; 16]),
        ];
    })
}

fn plain_layer(name: &str) -> TestLayer {
    TestLayer::pixels(name, 0, 0, 8, 8, 8, &rgba(8, 8, 3))
}

/// Eight by eight, bottom to top: a plain layer, two smart layers sharing
/// one embedded file, a smart layer with a second file, a plain one. The
/// list also holds a file no layer names.
fn smart_psd() -> TestPsd {
    TestPsd::new(1, 8, 8, 8).with(|p| {
        p.layers = vec![
            plain_layer("Background"),
            smart_layer("Smart A", ID_A, 1, 1, 10),
            smart_layer("Smart A copy", ID_A, 3, 2, 50),
            smart_layer("Smart B", ID_B, 2, 3, 90),
            plain_layer("Top"),
        ];
        p.globals = vec![(
            *b"lnk2",
            linked_list(&[
                (ID_A, payload(1, 301)),
                (UNUSED, payload(2, 77)),
                (ID_B, payload(3, 1024)),
            ]),
        )];
    })
}

fn linked_list(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    entries
        .iter()
        .flat_map(|(id, data)| linked_entry(id, data))
        .collect()
}

// ---------------------------------------------------------------------
// Opening and writing
// ---------------------------------------------------------------------

/// A PSD opened into a real store, as the app installs it.
struct Opened {
    _dir: tempfile::TempDir,
    store: TileStore,
    document: PsdDocument,
}

impl Opened {
    fn new(bytes: &[u8]) -> Self {
        let mut document = ok(read(bytes));
        let (dir, mut store) = store();
        for pixels in std::mem::take(&mut document.pixels) {
            let Some(surface) = document.layers.surface_id(pixels.layer) else {
                unreachable!("a pixel layer");
            };
            ok(crate::write_into_store_at(
                &pixels.image,
                &mut store,
                surface,
                pixels.offset.0,
                pixels.offset.1,
            ));
        }
        Self {
            _dir: dir,
            store,
            document,
        }
    }

    fn layer(&self, name: &str) -> LayerId {
        let mut found = None;
        for id in all_layers(&self.document.layers) {
            if self.document.layers.name(id) == Some(name) {
                found = Some(id);
            }
        }
        match found {
            Some(id) => id,
            None => unreachable!("no layer {name}"),
        }
    }

    fn surface(&self, name: &str) -> aurora_tile::SurfaceId {
        match self.document.layers.surface_id(self.layer(name)) {
            Some(s) => s,
            None => unreachable!("no surface"),
        }
    }

    fn write(&mut self) -> (Vec<u8>, PsdWriteOutcome) {
        let (w, h) = self.document.canvas_size;
        let composite = ok(Image::new(
            w,
            h,
            IccProfile::srgb(),
            vec![f16::from_f32(0.5); (w * h * 4) as usize],
        ));
        let mut out = Cursor::new(Vec::new());
        let mut source = &composite;
        let outcome = ok(write_psd(
            &mut out,
            &self.document.layers,
            self.document.canvas_size,
            &mut source,
            &mut self.store,
            PsdSaveOptions::default(),
        ));
        (out.into_inner(), outcome)
    }
}

fn bounds_of(tree: &LayerTree, id: LayerId) -> Rect {
    match tree.bounds(id) {
        Some(b) => b,
        None => unreachable!("a pixel layer"),
    }
}

/// Every layer id, top first, hidden ones and groups' children included.
fn all_layers(tree: &LayerTree) -> Vec<LayerId> {
    let mut out = Vec::new();
    let mut stack: Vec<LayerId> = tree.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        out.push(id);
        if let Some(aurora_doc::LayerKind::Group { children }) = tree.kind(id) {
            stack.extend(children.iter().rev().copied());
        }
    }
    out
}

fn smart_ids(tree: &LayerTree) -> Vec<(String, String)> {
    all_layers(tree)
        .into_iter()
        .filter_map(|id| {
            let so = tree.smart_object(id)?;
            Some((tree.name(id)?.to_owned(), so.unique_id.clone()))
        })
        .collect()
}

fn report_text(outcome: &PsdWriteOutcome) -> String {
    outcome.report.items.join("\n")
}

#[test]
fn the_reader_keeps_smart_objects_and_the_files_they_name() {
    let opened = Opened::new(&smart_psd().write());
    let tree = &opened.document.layers;
    assert_eq!(
        smart_ids(tree),
        vec![
            ("Smart B".to_owned(), ID_B.to_owned()),
            ("Smart A copy".to_owned(), ID_A.to_owned()),
            ("Smart A".to_owned(), ID_A.to_owned()),
        ],
        "paint order, top first"
    );
    let a = opened.layer("Smart A");
    let Some(so) = tree.smart_object(a) else {
        unreachable!("kept");
    };
    let keys: Vec<[u8; 4]> = so.blocks.iter().map(|b| b.key).collect();
    assert_eq!(keys, vec![*b"SoLd", *b"PlLd", *b"fxrp"]);
    assert_eq!(so.blocks[0].data, sold(ID_A), "verbatim");
    assert_eq!(so.blocks[1].data, plld(ID_A), "verbatim");
    assert_eq!(so.bounds, tree.bounds(a).unwrap_or(so.bounds));
    // Two files used; the third (no layer names it) was not read.
    let pool = tree.linked_files();
    assert_eq!(pool.len(), 2);
    assert_eq!(
        pool.get(ID_A).map(|f| f.entry.to_vec()),
        Some(linked_entry(ID_A, &payload(1, 301)))
    );
    assert_eq!(
        pool.get(ID_B).map(|f| f.entry.to_vec()),
        Some(linked_entry(ID_B, &payload(3, 1024)))
    );
    assert!(!pool.contains_key(UNUSED));
    assert!(
        opened
            .document
            .report
            .items
            .iter()
            .all(|i| !i.contains("smart object")),
        "kept ones are not a loss: {:?}",
        opened.document.report
    );
}

#[test]
fn the_open_time_fingerprint_is_the_one_the_store_computes() {
    let mut opened = Opened::new(&smart_psd().write());
    for name in ["Smart A", "Smart A copy", "Smart B"] {
        let id = opened.layer(name);
        let surface = opened.surface(name);
        let bounds = bounds_of(&opened.document.layers, id);
        let want = opened
            .document
            .layers
            .smart_object(id)
            .map(|s| s.pixel_fingerprint);
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
fn an_id_no_list_entry_carries_is_not_kept_and_is_reported() {
    let psd = smart_psd().with(|p| {
        p.globals = vec![(*b"lnk2", linked_list(&[(UNUSED, payload(2, 40))]))];
    });
    let opened = Opened::new(&psd.write());
    assert!(smart_ids(&opened.document.layers).is_empty());
    assert!(opened.document.layers.linked_files().is_empty());
    assert_eq!(
        opened.document.report.items,
        vec!["3 smart objects opened as pixels; the original contents aren't kept.".to_owned()]
    );
}

#[test]
fn a_matched_smart_layer_with_no_pixels_is_reported_and_pools_nothing() {
    // Smart B has an empty rectangle: it is the rendering of nothing.
    let mut psd = smart_psd();
    if let Some(layer) = psd.layers.get_mut(3) {
        *layer = TestLayer::empty("Smart B").with(|l| {
            l.blocks = vec![(*b"SoLd", sold(ID_B)), (*b"PlLd", plld(ID_B))];
        });
    }
    let opened = Opened::new(&psd.write());
    let names: Vec<String> = smart_ids(&opened.document.layers)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, vec!["Smart A copy", "Smart A"]);
    assert_eq!(
        opened.document.report.items,
        vec!["1 smart object opened as pixels; the original contents aren't kept.".to_owned()]
    );
    let pool: Vec<&String> = opened.document.layers.linked_files().keys().collect();
    assert_eq!(pool, vec![ID_A], "Smart B's file is not pooled");
}

#[test]
fn a_file_with_no_linked_lists_at_all_opens_as_before() {
    let psd = smart_psd().with(|p| p.globals.clear());
    let opened = Opened::new(&psd.write());
    assert!(smart_ids(&opened.document.layers).is_empty());
    assert_eq!(opened.document.report.items.len(), 1);
}

#[test]
fn smart_filters_keep_the_layer_a_plain_one() {
    let mut psd = smart_psd();
    if let Some(layer) = psd.layers.get_mut(1) {
        layer.blocks.push((*b"FXid", vec![0; 8]));
    }
    let opened = Opened::new(&psd.write());
    let names: Vec<String> = smart_ids(&opened.document.layers)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, vec!["Smart B", "Smart A copy"]);
    assert!(
        opened.document.report.items[0].starts_with("1 smart object opened as pixels"),
        "{:?}",
        opened.document.report
    );
}

#[test]
fn the_plld_id_is_used_when_the_descriptor_names_none() {
    let mut psd = smart_psd();
    if let Some(layer) = psd.layers.get_mut(1) {
        layer.blocks.retain(|(k, _)| k != b"SoLd");
    }
    let opened = Opened::new(&psd.write());
    let a = opened.layer("Smart A");
    assert_eq!(
        opened
            .document
            .layers
            .smart_object(a)
            .map(|s| s.unique_id.clone()),
        Some(ID_A.to_owned())
    );
}

#[test]
fn hostile_lists_are_survived_and_keep_nothing_they_cannot_locate() {
    let good = linked_entry(ID_A, &payload(1, 64));
    // A length that runs past the list.
    let mut lying = (u64::MAX / 2).to_be_bytes().to_vec();
    lying.extend_from_slice(&good[8..]);
    // A length that is zero, then the real entry.
    let mut zero = vec![0u8; 12];
    zero.extend_from_slice(&good);
    // Truncated in the middle of the ID.
    let cut = good[..14].to_vec();
    for (what, list) in [("lying", lying), ("zero", zero), ("cut", cut)] {
        let psd = smart_psd().with(|p| p.globals = vec![(*b"lnk2", list.clone())]);
        let opened = Opened::new(&psd.write());
        assert!(
            smart_ids(&opened.document.layers).is_empty(),
            "{what}: nothing located"
        );
    }
}

#[test]
fn a_second_list_key_is_read_too() {
    let psd = smart_psd().with(|p| {
        p.globals = vec![
            (*b"lnkD", linked_list(&[(ID_A, payload(1, 301))])),
            (*b"lnk3", linked_list(&[(ID_B, payload(3, 100))])),
        ];
    });
    let opened = Opened::new(&psd.write());
    let lists: Vec<[u8; 4]> = opened
        .document
        .layers
        .linked_files()
        .values()
        .map(|f| f.list)
        .collect();
    assert_eq!(lists, vec![*b"lnkD", *b"lnk3"]);
}

#[test]
fn an_unchanged_smart_object_round_trips_through_a_save() {
    let mut opened = Opened::new(&smart_psd().write());
    let (bytes, outcome) = opened.write();
    assert!(
        outcome.report.is_empty(),
        "lossless: {:?}",
        outcome.report.items
    );
    assert_eq!(outcome.expected.linked.len(), 2);

    // Our own parsers, on a real file.
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("out.psd");
    ok(std::fs::write(&path, &bytes));
    ok(verify_psd_file(&path, &outcome.expected));

    // And a reopen: same smart objects, byte-identical embedded files, and
    // the data is itself still true against the reopened pixels.
    let mut again = Opened::new(&bytes);
    assert_eq!(
        smart_ids(&again.document.layers),
        smart_ids(&opened.document.layers)
    );
    for id in [ID_A, ID_B] {
        assert_eq!(
            again
                .document
                .layers
                .linked_files()
                .get(id)
                .map(|f| f.entry.clone()),
            opened
                .document
                .layers
                .linked_files()
                .get(id)
                .map(|f| f.entry.clone()),
            "{id}"
        );
    }
    for name in ["Smart A", "Smart A copy", "Smart B"] {
        let a = opened.layer(name);
        let b = again.layer(name);
        let (Some(first), Some(second)) = (
            opened.document.layers.smart_object(a),
            again.document.layers.smart_object(b),
        ) else {
            unreachable!("{name} kept");
        };
        assert_eq!(first.blocks, second.blocks, "{name}");
    }
    // A second save of the reopened file is lossless again (stable).
    let (_, second) = again.write();
    assert!(second.report.is_empty(), "{:?}", second.report.items);
}

#[test]
fn only_the_files_a_written_layer_names_are_written() {
    let mut opened = Opened::new(&smart_psd().write());
    // Smart B goes plain: painting on it drops its file from the output.
    let b = opened.surface("Smart B");
    {
        let tile = ok(opened.store.get_mut(b, TileId { x: 0, y: 0 }));
        tile.texels_mut()[0] = f16::from_f32(0.25);
    }
    let (bytes, outcome) = opened.write();
    assert_eq!(outcome.expected.linked.len(), 1);
    assert_eq!(outcome.expected.linked[0].unique_id, ID_A);
    let again = ok(read(&bytes));
    assert_eq!(again.layers.linked_files().len(), 1);
}

fn dropped(opened: &mut Opened) -> (PsdWriteOutcome, LayerTree) {
    let (bytes, outcome) = opened.write();
    let again = ok(read(&bytes));
    (outcome, again.layers)
}

#[test]
fn painting_a_smart_object_layer_drops_it_and_says_so() {
    let mut opened = Opened::new(&smart_psd().write());
    let surface = opened.surface("Smart A");
    {
        let tile = ok(opened.store.get_mut(surface, TileId { x: 0, y: 0 }));
        tile.texels_mut()[3] = f16::from_f32(0.125);
    }
    let (outcome, tree) = dropped(&mut opened);
    let text = report_text(&outcome);
    assert!(text.contains("1 smart object layer will be saved as ordinary pixel layers"));
    assert!(text.contains("painted or erased"), "{text}");
    assert!(text.contains("\u{201c}Smart A\u{201d}"), "{text}");
    assert!(!text.contains("copy"), "only the edited layer: {text}");
    let names: Vec<String> = smart_ids(&tree).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["Smart B", "Smart A copy"]);
    // The shared file is still written, for the layer that kept it.
    assert!(tree.linked_files().contains_key(ID_A));
}

#[test]
fn erasing_to_nothing_and_putting_the_pixels_back_keeps_it() {
    let mut opened = Opened::new(&smart_psd().write());
    let surface = opened.surface("Smart B");
    let id = TileId { x: 0, y: 0 };
    let original = {
        let tile = ok(opened.store.get_mut(surface, id));
        let original = tile.texels().to_vec();
        tile.texels_mut().fill(f16::ZERO);
        original
    };
    let (outcome, _) = {
        let (bytes, outcome) = opened.write();
        (outcome, bytes)
    };
    assert!(report_text(&outcome).contains("Smart B"), "erased");
    {
        let tile = ok(opened.store.get_mut(surface, id));
        tile.texels_mut().copy_from_slice(&original);
    }
    let (outcome, tree) = dropped(&mut opened);
    assert!(outcome.report.is_empty(), "{:?}", outcome.report.items);
    assert_eq!(smart_ids(&tree).len(), 3, "an undone edit keeps it");
}

#[test]
fn moving_a_smart_object_layer_drops_it() {
    let mut opened = Opened::new(&smart_psd().write());
    let id = opened.layer("Smart A copy");
    let mut bounds = bounds_of(&opened.document.layers, id);
    bounds.x += 1;
    ok(opened.document.layers.set_bounds(id, bounds));
    let (outcome, tree) = dropped(&mut opened);
    let text = report_text(&outcome);
    assert!(text.contains("they were moved"), "{text}");
    assert!(text.contains("Smart A copy"), "{text}");
    assert_eq!(smart_ids(&tree).len(), 2);
}

#[test]
fn opacity_blend_visibility_locks_and_masks_keep_the_smart_object() {
    let mut opened = Opened::new(&smart_psd().write());
    let id = opened.layer("Smart A");
    let tree = &mut opened.document.layers;
    ok(tree.set_opacity(id, 0.5));
    ok(tree.set_fill_opacity(id, 0.25));
    ok(tree.set_blend_mode(id, aurora_doc::BlendMode::Multiply));
    ok(tree.set_visible(id, false));
    ok(tree.set_lock(id, LayerLock::all()));
    let bounds = bounds_of(tree, id);
    ok(tree.add_mask(id, bounds));
    let (outcome, tree) = dropped(&mut opened);
    let report = report_text(&outcome);
    assert_eq!(smart_ids(&tree).len(), 3);
    assert!(!report.contains("smart object"), "{report}");
}

#[test]
fn an_embedded_file_missing_from_the_pool_drops_the_layer() {
    let mut opened = Opened::new(&smart_psd().write());
    // A tree with the smart objects but without the pool, as a damaged
    // recovery might leave.
    let mut stripped = LayerTree::new();
    let mut history = History::new();
    let id = ok(history.add_pixel_layer(
        &mut stripped,
        "Lone",
        Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
        None,
    ));
    let Some(surface) = stripped.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let image = ok(Image::new(
        2,
        2,
        IccProfile::srgb(),
        vec![f16::from_f32(0.5); 16],
    ));
    ok(crate::write_into_store_at(
        &image,
        &mut opened.store,
        surface,
        0,
        0,
    ));
    let fingerprint = ok(aurora_doc::smart_object::surface_fingerprint(
        &mut opened.store,
        surface,
        bounds_of(&stripped, id),
    ));
    ok(stripped.set_smart_object(
        id,
        Some(aurora_doc::SmartObject {
            unique_id: ID_A.to_owned(),
            blocks: vec![aurora_doc::SmartObjectBlock {
                key: *b"SoLd",
                data: sold(ID_A),
            }],
            bounds: bounds_of(&stripped, id),
            pixel_fingerprint: fingerprint,
        }),
    ));
    let composite = ok(Image::new(
        8,
        8,
        IccProfile::srgb(),
        vec![f16::ONE; 8 * 8 * 4],
    ));
    let mut out = Cursor::new(Vec::new());
    let mut source = &composite;
    let outcome = ok(write_psd(
        &mut out,
        &stripped,
        (8, 8),
        &mut source,
        &mut opened.store,
        PsdSaveOptions::default(),
    ));
    let text = outcome.report.items.join("\n");
    assert!(
        text.contains("embedded file is no longer available"),
        "{text}"
    );
    assert!(outcome.expected.linked.is_empty());
}

#[test]
fn a_long_list_of_dropped_layers_is_summarised() {
    let mut opened = Opened::new(&smart_psd().write());
    for name in ["Smart A", "Smart A copy", "Smart B"] {
        let surface = opened.surface(name);
        let tile = ok(opened.store.get_mut(surface, TileId { x: 0, y: 0 }));
        tile.texels_mut()[5] = f16::from_f32(0.0625);
    }
    let (outcome, tree) = dropped(&mut opened);
    let text = report_text(&outcome);
    assert!(
        text.contains("3 smart object layers will be saved"),
        "{text}"
    );
    assert!(smart_ids(&tree).is_empty());
    assert!(tree.linked_files().is_empty(), "no lnk block at all");
}

// ---------------------------------------------------------------------
// .aur
// ---------------------------------------------------------------------

fn aur_bytes(opened: &mut Opened, history: &History) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write(
        &mut out,
        &opened.document.layers,
        history,
        opened.document.canvas_size,
        None,
        &crate::aur::SkippedTiles::new(),
        &mut opened.store,
    ));
    out.into_inner()
}

#[test]
fn smart_objects_survive_an_aur_round_trip_and_a_later_psd_save() {
    let mut opened = Opened::new(&smart_psd().write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let (_dir, mut store2) = store();
    let doc = ok(crate::aur::read(Cursor::new(&bytes), &mut store2));
    assert_eq!(smart_ids(&doc.layers), smart_ids(&opened.document.layers));
    assert_eq!(
        doc.layers.linked_files(),
        opened.document.layers.linked_files()
    );
    // Still true against the reloaded tiles.
    let mut reopened = Opened {
        _dir,
        store: store2,
        document: PsdDocument {
            layers: doc.layers,
            history: doc.history,
            canvas_size: doc.canvas_size,
            pixels: Vec::new(),
            masks: Vec::new(),
            report: super::PsdImportReport::default(),
        },
    };
    let (_, outcome) = reopened.write();
    assert!(outcome.report.is_empty(), "{:?}", outcome.report.items);
    assert_eq!(outcome.expected.linked.len(), 2);
}

#[test]
fn a_document_without_smart_objects_writes_no_smart_entries() {
    let psd = smart_psd().with(|p| p.globals.clear());
    let mut opened = Opened::new(&psd.write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let archive = ok(zip::ZipArchive::new(Cursor::new(&bytes)));
    assert!(
        archive
            .file_names()
            .filter_map(Result::ok)
            .all(|n| !n.starts_with("smart-object"))
    );
}

fn rewrite_entry(bytes: &[u8], name: &str, replacement: Option<&[u8]>) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut src = ok(zip::ZipArchive::new(Cursor::new(bytes)));
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..src.len() {
        let mut file = ok(src.by_index(i));
        let entry = String::from_utf8_lossy(file.name_raw()).into_owned();
        let mut data = Vec::new();
        ok(file.read_to_end(&mut data));
        if entry == name {
            let Some(new) = replacement else { continue };
            data = new.to_vec();
        }
        ok(out.start_file(entry, options));
        ok(out.write_all(&data));
    }
    ok(out.finish()).into_inner()
}

#[test]
fn damaged_or_newer_smart_entries_refuse_the_open() {
    let mut opened = Opened::new(&smart_psd().write());
    let history = std::mem::replace(&mut opened.document.history, History::new());
    let bytes = aur_bytes(&mut opened, &history);
    let open = |b: &[u8]| {
        let (_dir, mut s) = store();
        crate::aur::read(Cursor::new(b), &mut s).map(|_| ())
    };
    assert!(open(&bytes).is_ok());
    // A file entry gone.
    let gone = rewrite_entry(&bytes, "smart-object-file-1", None);
    assert!(matches!(
        open(&gone),
        Err(IoError::ManifestDeserialization(_))
    ));
    // A file entry of the right length whose frame is wrong (its length
    // field says something else).
    let mut sized = vec![0u8; 452];
    sized[..8].copy_from_slice(&7u64.to_be_bytes());
    let wrong_frame = rewrite_entry(&bytes, "smart-object-file-0", Some(&sized));
    assert!(matches!(
        open(&wrong_frame),
        Err(IoError::ManifestDeserialization(_))
    ));
    // A file entry that is not a framed linked-layer entry.
    let junk = rewrite_entry(&bytes, "smart-object-file-0", Some(&[1, 2, 3]));
    assert!(open(&junk).is_err());
    // A newer version.
    let mut wire = ok(zip::ZipArchive::new(Cursor::new(&bytes)));
    let mut meta = Vec::new();
    {
        use std::io::Read;
        ok(ok(wire.by_name("smart-objects")).read_to_end(&mut meta));
    }
    meta[0] = 2;
    let newer = rewrite_entry(&bytes, "smart-objects", Some(&meta));
    match open(&newer) {
        Err(IoError::ManifestDeserialization(msg)) => {
            assert!(msg.contains("newer Aurora"), "{msg}");
        }
        other => unreachable!("{other:?}"),
    }
    // No entry at all: opens, with no smart objects.
    let none = rewrite_entry(&bytes, "smart-objects", None);
    let (_dir, mut s) = store();
    let doc = ok(crate::aur::read(Cursor::new(&none), &mut s));
    assert!(smart_ids(&doc.layers).is_empty());
}

#[test]
fn the_autosave_snapshot_bound_counts_the_embedded_files() {
    let mut opened = Opened::new(&smart_psd().write());
    let pool: usize = opened
        .document
        .layers
        .linked_files()
        .values()
        .map(|f| f.entry.len())
        .sum();
    assert!(pool > 1000);
    let history = History::new();
    let snapshot = |opened: &mut Opened, budget: usize| {
        ok(crate::aur::snapshot_best_effort(
            &opened.document.layers,
            &history,
            opened.document.canvas_size,
            None,
            &crate::aur::SkippedTiles::new(),
            budget,
            &mut opened.store,
        ))
    };
    // The tiles alone fit a budget the files push it over.
    let tiles = snapshot(&mut opened, usize::MAX).map_or(0, |s| s.texel_bytes());
    assert!(snapshot(&mut opened, tiles + pool).is_some());
    assert!(
        snapshot(&mut opened, tiles + pool - 1).is_none(),
        "the pool is part of the bound"
    );
}

/// A document at the smart-object block bound (four 4 MiB blocks, 16 MiB
/// in all) still writes a journal the reader accepts: the blocks travel in
/// the `smart-objects` entry and again in the journal, with other ops and
/// a delete/undo carrier among them.
#[test]
fn a_document_at_the_block_bound_round_trips_through_aur_with_other_journal_ops() {
    let (_dir, mut store1) = store();
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let bounds = Rect {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    };
    let per_block = aurora_doc::smart_object::MAX_SMART_OBJECT_BLOCK_BYTES;
    let layers = aurora_doc::smart_object::MAX_SMART_OBJECT_TOTAL_BLOCK_BYTES / per_block;
    assert_eq!(layers, 4);
    let mut ids = Vec::new();
    for i in 0..layers {
        let id = ok(history.add_pixel_layer(&mut tree, format!("S{i}"), bounds, None));
        ok(tree.add_linked_file(aurora_doc::LinkedFile {
            list: *b"lnk2",
            unique_id: format!("id-{i}"),
            entry: std::sync::Arc::new(linked_entry(&format!("id-{i}"), &payload(i as u8, 64))),
        }));
        ok(history.set_smart_object(
            &mut tree,
            id,
            Some(aurora_doc::SmartObject {
                unique_id: format!("id-{i}"),
                blocks: vec![aurora_doc::SmartObjectBlock {
                    key: *b"SoLd",
                    data: vec![i as u8 + 1; per_block],
                }],
                bounds,
                pixel_fingerprint: 0,
            }),
        ));
        ids.push(id);
    }
    // Other journal ops: an opacity edit, and a delete + undo (a carrier
    // for the deleted layer's smart object).
    ok(history.set_opacity(&mut tree, ids[0], 0.5));
    ok(history.remove(&mut tree, ids[1]));
    ok(history.undo(&mut tree));
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write(
        &mut out,
        &tree,
        &history,
        (8, 8),
        None,
        &crate::aur::SkippedTiles::new(),
        &mut store1,
    ));
    let (_dir2, mut store2) = store();
    let doc = ok(crate::aur::read(Cursor::new(out.into_inner()), &mut store2));
    for (i, id) in ids.iter().enumerate() {
        let got = doc.layers.smart_object(*id);
        assert_eq!(got.map(|s| s.blocks[0].data.len()), Some(per_block), "{i}");
        assert_eq!(got.map(|s| s.blocks[0].data[0]), Some(i as u8 + 1), "{i}");
    }
    assert_eq!(doc.layers.linked_files().len(), layers);
}

// ---------------------------------------------------------------------
// The user's template
// ---------------------------------------------------------------------

#[test]
fn the_nyepi_template_round_trips_its_35_smart_objects() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../images/flat-design-nyepi-template/6739968.psd");
    if !path.exists() {
        println!("SKIPPED: {} is not present", path.display());
        return;
    }
    let bytes = ok(std::fs::read(&path));
    let mut opened = Opened::new(&bytes);
    let tree = &opened.document.layers;
    let count = smart_ids(tree).len();
    assert_eq!(count, 35, "every smart object kept");
    assert!(
        opened
            .document
            .report
            .items
            .iter()
            .all(|i| !i.contains("smart object")),
        "{:?}",
        opened.document.report.items
    );
    let originals: Vec<(String, std::sync::Arc<Vec<u8>>)> = tree
        .linked_files()
        .iter()
        .map(|(id, f)| (id.clone(), f.entry.clone()))
        .collect();
    println!(
        "{count} smart objects, {} embedded files, {} bytes",
        originals.len(),
        originals.iter().map(|(_, e)| e.len()).sum::<usize>()
    );
    let (out_bytes, outcome) = opened.write();
    let dropped_lines: Vec<&String> = outcome
        .report
        .items
        .iter()
        .filter(|i| i.contains("smart object"))
        .collect();
    assert!(dropped_lines.is_empty(), "{dropped_lines:?}");
    let dir = ok(tempfile::tempdir());
    let out = dir.path().join("nyepi-out.psd");
    ok(std::fs::write(&out, &out_bytes));
    ok(verify_psd_file(&out, &outcome.expected));
    let again = ok(read(&out_bytes));
    assert_eq!(smart_ids(&again.layers).len(), 35, "reopened");
    assert_eq!(again.layers.linked_files().len(), originals.len());
    for (id, entry) in &originals {
        assert_eq!(
            again.layers.linked_files().get(id).map(|f| f.entry.clone()),
            Some(entry.clone()),
            "{id} is byte-identical"
        );
    }
}

/// Writes the files `scripts/verify_psd_write.py` checks with psd-tools
/// (`AURORA_PSD_FIXTURE_DIR`): `smart.psd` + `smart.json` (the synthetic
/// document, `SoLd` only — psd-tools needs a full `PlLd` to parse one and
/// the fixture has no placement data) and, when the user's template is
/// present, `nyepi-roundtrip.psd` + `nyepi.json`.
#[test]
#[ignore = "writes fixtures for scripts/verify_psd_write.py"]
fn write_smart_fixtures_for_psd_tools() {
    let Ok(out) = std::env::var("AURORA_PSD_FIXTURE_DIR") else {
        unreachable!("set AURORA_PSD_FIXTURE_DIR");
    };
    let out = std::path::Path::new(&out);
    let mut psd = smart_psd();
    for layer in &mut psd.layers {
        layer.blocks.retain(|(k, _)| k == b"SoLd");
    }
    let mut opened = Opened::new(&psd.write());
    let (bytes, outcome) = opened.write();
    assert!(outcome.report.is_empty(), "{:?}", outcome.report.items);
    ok(std::fs::write(out.join("smart.psd"), &bytes));
    let hex = |data: &[u8]| -> String { data.iter().map(|b| format!("{b:02x}")).collect() };
    let layers: Vec<String> = [
        ("Smart A", ID_A, payload(1, 301)),
        ("Smart A copy", ID_A, payload(1, 301)),
        ("Smart B", ID_B, payload(3, 1024)),
    ]
    .iter()
    .map(|(name, id, data)| {
        format!(
            "{{\"name\":{name:?},\"unique_id\":{id:?},\"data\":\"{}\"}}",
            hex(data)
        )
    })
    .collect();
    ok(std::fs::write(
        out.join("smart.json"),
        format!("{{\"smart\":[{}]}}", layers.join(",")),
    ));

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../images/flat-design-nyepi-template/6739968.psd");
    if path.exists() {
        let mut nyepi = Opened::new(&ok(std::fs::read(&path)));
        let (bytes, _) = nyepi.write();
        ok(std::fs::write(out.join("nyepi-roundtrip.psd"), bytes));
        ok(std::fs::write(
            out.join("nyepi.json"),
            format!("{{\"original\":{:?}}}", path.display().to_string()),
        ));
    }
}

#[test]
fn verification_notices_a_damaged_embedded_file_and_a_missing_one() {
    let mut opened = Opened::new(&smart_psd().write());
    let (bytes, outcome) = opened.write();
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("out.psd");
    ok(std::fs::write(&path, &bytes));
    ok(verify_psd_file(&path, &outcome.expected));
    // One byte inside the embedded payload of Smart B (a run of known
    // bytes: its payload).
    let needle = payload(3, 1024);
    let Some(at) = bytes.windows(64).position(|w| w == &needle[..64]) else {
        unreachable!("the payload is in the file");
    };
    let mut damaged = bytes.clone();
    damaged[at + 10] ^= 0xFF;
    ok(std::fs::write(&path, &damaged));
    assert!(matches!(
        verify_psd_file(&path, &outcome.expected),
        Err(IoError::PsdVerifyFailed { .. })
    ));
    // The file as written, against an expectation with one file too many.
    let mut more = outcome.expected.clone();
    more.linked.push(more.linked[0].clone());
    ok(std::fs::write(&path, &bytes));
    assert!(verify_psd_file(&path, &more).is_err());
}
