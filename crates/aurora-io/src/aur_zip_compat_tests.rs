//! Compatibility of the `.aur` container across the `zip` crate upgrade
//! (2.4.2 to 9.0.3, task 109).
//!
//! `tests/fixtures/aur/*.aur` were written by the **old** build (zip
//! 2.4.2) with [`regenerate_fixtures`] and committed as binaries. These
//! tests open every one of them with whatever `zip` the workspace now
//! uses and check the document structure, a few pixels, the entry table
//! (names, order, compression method, CRC, uncompressed size) and that a
//! fresh write of the same document produces the same entry table and the
//! same uncompressed bytes of every entry. They do not claim that the
//! whole file is byte identical to the old one: the ZIP envelope (local
//! headers, timestamps, the compressed deflate stream) is the crate's.
//!
//! Regenerating (only ever needed if a fixture is deliberately changed):
//! `AURORA_ZIP_FIXTURE_OUT=<dir> cargo test -p aurora-io regenerate_fixtures -- --ignored`
//! then copy the files by hand; a plain run never writes into the tree.

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use aurora_core::{LayerEffects, Rect};
use aurora_doc::{
    Adjustment, Anchor, BlendMode, History, LayerKind, LayerTree, LinkedFile, Path, PathSet,
    Selection, SelectionMask, SelectionSet, SmartObject, SmartObjectBlock, Subpath, WORK_PATH_NAME,
};
use aurora_tile::{TILE, TileId, TileStore};
use half::f16;
use zip::{CompressionMethod, ZipArchive};

use crate::aur::{AurDocument, SkippedTiles, read, snapshot_best_effort, write_snapshot};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(32) else {
        unreachable!("non-zero")
    };
    let store = ok(TileStore::new(dir.path().to_path_buf(), budget));
    (dir, store)
}

fn r(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("aur")
        .join(name)
}

fn fixture(name: &str) -> Vec<u8> {
    ok(std::fs::read(fixture_path(name)))
}

/// Writes one texel of a layer's tile (RGBA as given).
fn put(store: &mut TileStore, surface: aurora_tile::SurfaceId, id: TileId, at: usize, v: [f32; 4]) {
    let tile = ok(store.get_mut(surface, id));
    let texels = tile.texels_mut();
    for (i, c) in v.iter().enumerate() {
        if let Some(slot) = texels.get_mut(at * 4 + i) {
            *slot = f16::from_f32(*c);
        }
    }
}

fn get(store: &mut TileStore, surface: aurora_tile::SurfaceId, id: TileId, at: usize) -> [f32; 4] {
    let tile = ok(store.get(surface, id));
    let t = tile.texels();
    let mut out = [0.0_f32; 4];
    for (i, o) in out.iter_mut().enumerate() {
        *o = t.get(at * 4 + i).map_or(f32::NAN, |c| c.to_f32());
    }
    out
}

fn surface_of(tree: &LayerTree, id: aurora_doc::LayerId) -> aurora_tile::SurfaceId {
    match tree.surface_id(id) {
        Some(s) => s,
        None => unreachable!("a pixel layer has a surface"),
    }
}

// ---------------------------------------------------------------------
// The documents. Deterministic: the same call gives the same bytes.
// ---------------------------------------------------------------------

/// One layer, no painted tile: the container has no tile entry at all.
fn build_empty() -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut tree = LayerTree::new();
    let mut history = History::new();
    ok(history.add_pixel_layer(&mut tree, "Empty", r(0, 0, 64, 64), None));
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write(
        &mut out,
        &tree,
        &history,
        (64, 64),
        None,
        &SkippedTiles::new(),
        &mut store,
    ));
    out.into_inner()
}

fn sample_path(name: &str, seed: f32) -> Path {
    Path {
        name: name.to_owned(),
        subpaths: vec![
            Subpath {
                closed: true,
                anchors: vec![
                    Anchor::corner(seed, 2.5),
                    Anchor::smooth((10.25 + seed, 3.0), (4.0, -1.5)),
                    Anchor::corner(-3.0, 40.0),
                ],
            },
            Subpath {
                closed: false,
                anchors: vec![Anchor::corner(0.0, 0.0), Anchor::corner(1.0, 1.0)],
            },
        ],
    }
}

fn rich_selection() -> SelectionSet {
    let mask_bounds = r(2, 3, 9, 7);
    let data: Vec<u8> = (0..mask_bounds.area_px())
        .map(|i| u8::try_from((i * 7 + 5) % 256).unwrap_or(0))
        .collect();
    let mask = Selection::from_mask(Arc::new(ok(SelectionMask::new(mask_bounds, data))));
    let mut set = SelectionSet::new();
    set.select(Selection::new(r(1, 2, 20, 12)));
    ok(set.save_active("Box"));
    set.select(Selection::ellipse(r(4, 4, 30, 18)));
    ok(set.save_active("Oval"));
    set.select(mask);
    set.set_paths(ok(PathSet::from_parts(
        vec![sample_path("Outline", 1.0), sample_path("Zeta", 7.5)],
        Some(sample_path(WORK_PATH_NAME, 2.0)),
    )));
    set
}

/// Pixel layers with painted tiles, a group, a mask, a clipped layer, a
/// Curves layer, effects, a smart object and its embedded file (a
/// **Stored** entry), import losses, saved selections and paths.
fn build_rich() -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let canvas = r(0, 0, 300, 100);
    let base = ok(history.add_pixel_layer(&mut tree, "Base", canvas, None));
    let top = ok(history.add_pixel_layer(&mut tree, "Top", canvas, None));
    let group = ok(history.add_group(&mut tree, "Group", None));
    let curves = ok(history.add_adjustment_layer_at(
        &mut tree,
        "Curves 1",
        Adjustment::Curves(aurora_core::CurvesParams::identity()),
        Some(group),
        0,
    ));
    ok(history.set_blend_mode(&mut tree, top, BlendMode::Multiply));
    ok(history.set_opacity(&mut tree, top, 0.5));
    ok(history.set_clipped(&mut tree, top, true));
    ok(history.add_mask(&mut tree, &mut store, base, r(10, 10, 40, 30)));
    let fx = LayerEffects {
        drop_shadows: vec![aurora_core::effects::DropShadow {
            distance: 6.0,
            ..aurora_core::effects::DropShadow::default()
        }],
        ..LayerEffects::default()
    };
    ok(history.set_effects(&mut tree, base, Some(fx)));
    ok(tree.add_linked_file(LinkedFile {
        list: *b"lnkD",
        unique_id: "so-1".to_owned(),
        entry: Arc::new(framed_file()),
    }));
    ok(history.set_smart_object(
        &mut tree,
        top,
        Some(SmartObject {
            unique_id: "so-1".to_owned(),
            blocks: vec![SmartObjectBlock {
                key: *b"SoLd",
                data: vec![7; 12],
            }],
            bounds: canvas,
            pixel_fingerprint: 0x1234_5678_9abc_def0,
        }),
    ));
    let base_surface = surface_of(&tree, base);
    put(&mut store, base_surface, TileId { x: 0, y: 0 }, 0, [1.0, 0.5, 0.25, 1.0]);
    put(&mut store, base_surface, TileId { x: 1, y: 0 }, 5, [0.0, 1.0, 0.0, 1.0]);
    let top_surface = surface_of(&tree, top);
    put(&mut store, top_surface, TileId { x: 0, y: 0 }, 1, [0.25, 0.25, 0.25, 0.5]);
    let _ = curves;
    let losses = vec![
        "Opened with changes: line one.".to_owned(),
        "Second line, with a non-ASCII byte: \u{e9}.".to_owned(),
    ];
    let set = rich_selection();
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write_with_import_losses(
        &mut out,
        &tree,
        &history,
        (300, 100),
        None,
        &SkippedTiles::new(),
        Some(&losses),
        Some(&set),
        &mut store,
    ));
    out.into_inner()
}

fn framed_file() -> Vec<u8> {
    // u64 big-endian body length, the body, zero padding to four bytes.
    let body: Vec<u8> = (0..13_u8).map(|b| b.wrapping_mul(17)).collect();
    let mut out = (body.len() as u64).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0; 3]);
    out
}

/// A 700 x 300 layer: 3 x 2 tiles, five of them with smooth gradient
/// content and one filled with incompressible-ish pseudo-random bytes.
fn build_large() -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let id = ok(history.add_pixel_layer(&mut tree, "Large", r(0, 0, 700, 300), None));
    let surface = surface_of(&tree, id);
    let mut seed: u32 = 0x2545_F491;
    for ty in 0..2_u32 {
        for tx in 0..3_u32 {
            let tile = ok(store.get_mut(surface, TileId { x: tx, y: ty }));
            let texels = tile.texels_mut();
            let noisy = tx == 2 && ty == 1;
            for (i, slot) in texels.iter_mut().enumerate() {
                *slot = if noisy {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    f16::from_f32((seed & 0xFFFF) as f32 / 65535.0)
                } else {
                    let texel = i / 4;
                    let v = ((texel % TILE as usize) as f32) / 255.0;
                    f16::from_f32(if i % 4 == 3 { 1.0 } else { v })
                };
            }
        }
    }
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write(
        &mut out,
        &tree,
        &history,
        (700, 300),
        None,
        &SkippedTiles::new(),
        &mut store,
    ));
    out.into_inner()
}

/// The autosave path: a snapshot taken on the UI thread, written by
/// `write_snapshot`, with a selection and paths.
fn build_snapshot() -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut tree = LayerTree::new();
    let mut history = History::new();
    let a = ok(history.add_pixel_layer(&mut tree, "Snap A", r(0, 0, 300, 100), None));
    let b = ok(history.add_pixel_layer(&mut tree, "Snap B", r(0, 0, 300, 100), None));
    put(&mut store, surface_of(&tree, a), TileId { x: 0, y: 0 }, 3, [0.5, 0.5, 0.0, 1.0]);
    put(&mut store, surface_of(&tree, b), TileId { x: 1, y: 0 }, 9, [0.0, 0.5, 0.5, 0.75]);
    let snapshot = ok(snapshot_best_effort(
        &tree,
        &history,
        (300, 100),
        None,
        &SkippedTiles::new(),
        usize::MAX,
        &mut store,
    ));
    let Some(mut snapshot) = snapshot else {
        unreachable!("unbounded budget gives a snapshot")
    };
    snapshot.set_selection(Some(&rich_selection()));
    let mut out = Cursor::new(Vec::new());
    let skipped = ok(write_snapshot(&mut out, &snapshot));
    assert!(skipped.is_empty());
    out.into_inner()
}

/// The same container with every entry rewritten by the zip crate with
/// one chosen method: a reader must not care which method wrote what.
fn rewritten_with(bytes: &[u8], method: CompressionMethod) -> Vec<u8> {
    let mut src = ok(ZipArchive::new(Cursor::new(bytes)));
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(method);
    for index in 0..src.len() {
        let mut file = ok(src.by_index(index));
        let name = file.name().to_owned();
        let mut data = Vec::new();
        ok(file.read_to_end(&mut data));
        ok(out.start_file(name, options));
        ok(out.write_all(&data));
    }
    ok(out.finish()).into_inner()
}

fn build_all() -> Vec<(&'static str, Vec<u8>)> {
    let painted = build_snapshot();
    vec![
        ("empty.aur", build_empty()),
        ("rich.aur", build_rich()),
        ("large.aur", build_large()),
        ("snapshot.aur", build_snapshot()),
        (
            "all-stored.aur",
            rewritten_with(&painted, CompressionMethod::Stored),
        ),
        (
            "all-deflated.aur",
            rewritten_with(&painted, CompressionMethod::Deflated),
        ),
    ]
}

/// Writes the fixtures into `$AURORA_ZIP_FIXTURE_OUT`. Ignored: run once
/// on the OLD build.
#[test]
#[ignore = "writes fixtures; run only on the zip 2.4.2 build"]
fn regenerate_fixtures() {
    let Some(dir) = std::env::var_os("AURORA_ZIP_FIXTURE_OUT") else {
        unreachable!("set AURORA_ZIP_FIXTURE_OUT to the output directory");
    };
    let dir = PathBuf::from(dir);
    ok(std::fs::create_dir_all(&dir));
    for (name, bytes) in build_all() {
        ok(std::fs::write(dir.join(name), bytes));
    }
}

// ---------------------------------------------------------------------
// The entry table
// ---------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone)]
struct EntryInfo {
    name: String,
    method: CompressionMethod,
    crc32: u32,
    size: u64,
    /// SHA-less content check: the entry's own uncompressed bytes.
    data: Vec<u8>,
}

fn entry_table(bytes: &[u8]) -> Vec<EntryInfo> {
    let mut archive = ok(ZipArchive::new(Cursor::new(bytes)));
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let mut file = ok(archive.by_index(index));
        let mut data = Vec::new();
        ok(file.read_to_end(&mut data));
        out.push(EntryInfo {
            name: file.name().to_owned(),
            method: file.compression(),
            crc32: file.crc32(),
            size: file.size(),
            data,
        });
    }
    out
}

fn open(bytes: &[u8]) -> (tempfile::TempDir, TileStore, AurDocument) {
    let (dir, mut store) = store();
    let doc = ok(read(Cursor::new(bytes.to_vec()), &mut store));
    (dir, store, doc)
}

fn layer_named(doc: &AurDocument, name: &str) -> aurora_doc::LayerId {
    let mut stack: Vec<aurora_doc::LayerId> = doc.layers.roots().to_vec();
    while let Some(id) = stack.pop() {
        if doc.layers.name(id) == Some(name) {
            return id;
        }
        if let Some(children) = doc.layers.children(id) {
            stack.extend_from_slice(children);
        }
    }
    unreachable!("no layer named {name}")
}

#[allow(clippy::float_cmp)]
fn assert_px(got: [f32; 4], want: [f32; 4]) {
    assert_eq!(got, want);
}

// ---------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------

#[test]
fn fixtures_total_under_two_megabytes() {
    let mut total = 0;
    for (name, _) in build_all() {
        total += fixture(name).len();
    }
    assert!(total < 2 * 1024 * 1024, "fixtures are {total} bytes");
}

#[test]
fn every_fixture_starts_with_the_stored_mimetype_entry_first() {
    for (name, _) in build_all() {
        let table = entry_table(&fixture(name));
        if name.starts_with("all-") {
            // Rewritten with one method for every entry on purpose.
            continue;
        }
        let Some(first) = table.first() else {
            unreachable!("{name} is empty")
        };
        assert_eq!(first.name, "mimetype", "{name}");
        assert_eq!(first.method, CompressionMethod::Stored, "{name}");
        assert_eq!(first.data, b"application/vnd.aurora.document", "{name}");
    }
}

#[test]
fn old_empty_fixture_opens() {
    let bytes = fixture("empty.aur");
    let names: Vec<String> = entry_table(&bytes).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["mimetype", "manifest", "history"]);
    let (_d, _s, doc) = open(&bytes);
    assert_eq!(doc.canvas_size, (64, 64));
    assert_eq!(doc.layers.len(), 1);
    let id = layer_named(&doc, "Empty");
    assert!(matches!(doc.layers.kind(id), Some(LayerKind::Pixel { .. })));
    assert!(doc.skipped_tiles.is_empty());
    assert!(doc.import_losses.is_none());
    assert!(doc.selection.active().is_none());
}

#[test]
fn old_rich_fixture_opens_with_every_optional_entry() {
    let bytes = fixture("rich.aur");
    let table = entry_table(&bytes);
    let names: Vec<&str> = table.iter().map(|e| e.name.as_str()).collect();
    for want in [
        "mimetype",
        "manifest",
        "history",
        "layer-effects",
        "layer-clipping",
        "smart-objects",
        "smart-object-file-0",
        "import-losses",
        "selection",
        "paths",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let methods: std::collections::HashMap<&str, CompressionMethod> =
        table.iter().map(|e| (e.name.as_str(), e.method)).collect();
    assert_eq!(methods.get("mimetype"), Some(&CompressionMethod::Stored));
    assert_eq!(
        methods.get("smart-object-file-0"),
        Some(&CompressionMethod::Stored)
    );
    assert_eq!(methods.get("manifest"), Some(&CompressionMethod::Deflated));
    assert_eq!(methods.get("history"), Some(&CompressionMethod::Deflated));

    let (_d, mut store, doc) = open(&bytes);
    assert_eq!(doc.canvas_size, (300, 100));
    let base = layer_named(&doc, "Base");
    let top = layer_named(&doc, "Top");
    let group = layer_named(&doc, "Group");
    let curves = layer_named(&doc, "Curves 1");
    assert!(matches!(doc.layers.kind(group), Some(LayerKind::Group { .. })));
    assert_eq!(doc.layers.parent(curves), Some(group));
    assert!(matches!(
        doc.layers.adjustment(curves),
        Some(Adjustment::Curves(_))
    ));
    assert_eq!(doc.layers.blend_mode(top), Some(BlendMode::Multiply));
    assert_eq!(doc.layers.opacity(top), Some(0.5));
    assert!(doc.layers.clipped(top));
    assert!(doc.layers.mask(base).is_some());
    assert!(doc.layers.effects(base).is_some());
    assert_eq!(
        doc.layers.smart_object(top).map(|s| s.unique_id.as_str()),
        Some("so-1")
    );
    assert_eq!(
        doc.layers.linked_files().get("so-1").map(|f| f.entry.as_ref()),
        Some(&framed_file())
    );
    assert_px(
        get(&mut store, surface_of(&doc.layers, base), TileId { x: 0, y: 0 }, 0),
        [1.0, 0.5, 0.25, 1.0],
    );
    assert_px(
        get(&mut store, surface_of(&doc.layers, base), TileId { x: 1, y: 0 }, 5),
        [0.0, 1.0, 0.0, 1.0],
    );
    assert_px(
        get(&mut store, surface_of(&doc.layers, top), TileId { x: 0, y: 0 }, 1),
        [0.25, 0.25, 0.25, 0.5],
    );
    assert_eq!(doc.import_losses.as_ref().map(Vec::len), Some(2));
    assert!(doc.selection.active().is_some());
    assert_eq!(doc.selection.saved_count(), 2);
    assert_eq!(doc.selection.paths().saved().len(), 2);
    assert!(doc.selection.paths().work().is_some());
}

#[test]
fn old_large_fixture_opens_and_keeps_its_tiles() {
    let bytes = fixture("large.aur");
    let table = entry_table(&bytes);
    let tiles = table.iter().filter(|e| e.name.starts_with("tile")).count();
    assert!(tiles >= 6, "{tiles} tile entries");
    assert!(table.iter().any(|e| e.size > 400 * 1024), "a big entry");
    let (_d, mut store, doc) = open(&bytes);
    assert_eq!(doc.canvas_size, (700, 300));
    let id = layer_named(&doc, "Large");
    let s = surface_of(&doc.layers, id);
    // A gradient tile: red channel of texel 17 is 17/255 (as an f16).
    let want = f16::from_f32(17.0 / 255.0).to_f32();
    assert_px(
        get(&mut store, s, TileId { x: 1, y: 0 }, 17),
        [want, want, want, 1.0],
    );
    // The noisy tile came back bit for bit: compare with a fresh build.
    let (_d2, mut store2, doc2) = open(&build_large());
    let id2 = layer_named(&doc2, "Large");
    let s2 = surface_of(&doc2.layers, id2);
    for at in [0_usize, 1, 999, 40_000, 65_535] {
        let a = get(&mut store, s, TileId { x: 2, y: 1 }, at);
        let b = get(&mut store2, s2, TileId { x: 2, y: 1 }, at);
        assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
    }
}

#[test]
fn old_autosave_snapshot_fixture_opens() {
    let bytes = fixture("snapshot.aur");
    let (_d, mut store, doc) = open(&bytes);
    assert_eq!(doc.canvas_size, (300, 100));
    let a = layer_named(&doc, "Snap A");
    let b = layer_named(&doc, "Snap B");
    assert_px(
        get(&mut store, surface_of(&doc.layers, a), TileId { x: 0, y: 0 }, 3),
        [0.5, 0.5, 0.0, 1.0],
    );
    assert_px(
        get(&mut store, surface_of(&doc.layers, b), TileId { x: 1, y: 0 }, 9),
        [0.0, 0.5, 0.5, 0.75],
    );
    assert!(doc.selection.active().is_some());
    assert_eq!(doc.selection.saved_count(), 2);
    assert_eq!(doc.selection.paths().saved().len(), 2);
}

#[test]
fn a_reader_does_not_care_which_method_wrote_which_entry() {
    for (name, method) in [
        ("all-stored.aur", CompressionMethod::Stored),
        ("all-deflated.aur", CompressionMethod::Deflated),
    ] {
        let bytes = fixture(name);
        let table = entry_table(&bytes);
        assert!(table.iter().all(|e| e.method == method), "{name}");
        let (_d, mut store, doc) = open(&bytes);
        let a = layer_named(&doc, "Snap A");
        assert_px(
            get(&mut store, surface_of(&doc.layers, a), TileId { x: 0, y: 0 }, 3),
            [0.5, 0.5, 0.0, 1.0],
        );
        assert_eq!(doc.selection.saved_count(), 2);
    }
}

/// The entry table of a fresh write equals the old one entry by entry:
/// same names in the same order, same method, same CRC-32, same size and
/// the same uncompressed bytes.
#[test]
fn a_fresh_write_has_the_old_entry_table() {
    for (name, fresh) in build_all() {
        let old = entry_table(&fixture(name));
        let new = entry_table(&fresh);
        assert_eq!(old.len(), new.len(), "{name} entry count");
        for (o, n) in old.iter().zip(&new) {
            assert_eq!(o.name, n.name, "{name} order");
            assert_eq!(o.method, n.method, "{name}/{}", o.name);
            assert_eq!(o.crc32, n.crc32, "{name}/{}", o.name);
            assert_eq!(o.size, n.size, "{name}/{}", o.name);
            assert_eq!(o.data, n.data, "{name}/{} bytes", o.name);
        }
    }
}

/// What the container's envelope promises, read from the **raw** central
/// directory so the crate under test is not the judge: signature, the
/// entry count, and no ZIP64 record for a small file.
#[test]
fn small_files_are_plain_zip_not_zip64() {
    for (name, _) in build_all() {
        let bytes = fixture(name);
        assert_eq!(bytes.get(..4), Some(&b"PK\x03\x04"[..]), "{name}");
        let fresh = build_all()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| b)
            .unwrap_or_default();
        for (label, file) in [("old", &bytes), ("new", &fresh)] {
            let has = |sig: &[u8]| file.windows(sig.len()).any(|w| w == sig);
            assert!(!has(b"PK\x06\x06"), "{name} {label}: zip64 EOCD record");
            assert!(!has(b"PK\x06\x07"), "{name} {label}: zip64 EOCD locator");
        }
    }
}

#[test]
fn every_fixture_round_trips_through_the_current_writer() {
    for (name, _) in build_all() {
        let bytes = fixture(name);
        let (_d, mut store, doc) = open(&bytes);
        let mut out = Cursor::new(Vec::new());
        ok(crate::aur::write_with_import_losses(
            &mut out,
            &doc.layers,
            &doc.history,
            doc.canvas_size,
            doc.profile.as_ref(),
            &doc.skipped_tiles,
            doc.import_losses.as_deref(),
            Some(&doc.selection),
            &mut store,
        ));
        let again = out.into_inner();
        let (_d2, _s2, doc2) = open(&again);
        assert_eq!(doc2.canvas_size, doc.canvas_size, "{name}");
        assert_eq!(doc2.layers.len(), doc.layers.len(), "{name}");
        assert_eq!(doc2.selection.saved_count(), doc.selection.saved_count());
        // And the entries (names, order, methods) are the old file's.
        let old: Vec<(String, CompressionMethod)> = entry_table(&bytes)
            .into_iter()
            .map(|e| (e.name, e.method))
            .collect();
        let new: Vec<(String, CompressionMethod)> = entry_table(&again)
            .into_iter()
            .map(|e| (e.name, e.method))
            .collect();
        if name != "all-stored.aur" && name != "all-deflated.aur" && name != "snapshot.aur" {
            assert_eq!(old, new, "{name}");
        }
    }
}

#[test]
fn write_is_deterministic_across_two_calls() {
    assert_eq!(entry_table(&build_rich()), entry_table(&build_rich()));
}
