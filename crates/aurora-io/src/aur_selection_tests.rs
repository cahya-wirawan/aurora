//! Tests of the `selection` entry (0.218.0): round trips for every shape,
//! the caps and the refusal/drop split, old files, and the snapshot path.

use std::io::Cursor;
use std::sync::Arc;

use aurora_core::Rect;
use aurora_doc::{History, LayerTree, Selection, SelectionMask, SelectionSet, SelectionShape};
use aurora_tile::TileStore;

use super::{
    KIND_ELLIPSE, KIND_MASK, KIND_RECT, SELECTION_ENTRY, SELECTION_VERSION, SelectionHeader,
    SelectionMeta, SelectionRead, decode_selection, encode_selection,
};
use crate::aur::{AurDocument, SkippedTiles, read, snapshot_best_effort, write_snapshot};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn store() -> (tempfile::TempDir, TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(8) else {
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

fn tree() -> LayerTree {
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer("A", r(0, 0, 4, 4), None));
    tree
}

fn mask_selection(bounds: Rect, seed: u8, inverted: bool) -> Selection {
    let n = bounds.area_px();
    let data: Vec<u8> = (0..n)
        .map(|i| u8::try_from((i * 7 + u64::from(seed)) % 256).unwrap_or(0))
        .collect();
    let mut selection = Selection::from_mask(Arc::new(ok(SelectionMask::new(bounds, data))));
    selection.inverted = inverted;
    selection
}

fn rich_set() -> SelectionSet {
    let mut inverted_ellipse = Selection::ellipse(r(-5, 3, 20, 11));
    inverted_ellipse.inverted = true;
    let mut set = ok(SelectionSet::from_parts(
        Some(mask_selection(r(-7, 2, 33, 17), 3, true)),
        vec![
            ("Plain".to_owned(), Selection::new(r(1, 2, 3, 4))),
            ("Oval".to_owned(), inverted_ellipse),
            (
                "Hair".to_owned(),
                mask_selection(r(10, 10, 9, 9), 99, false),
            ),
            (
                "Everything but nothing".to_owned(),
                Selection {
                    bounds: r(3, 3, 0, 7),
                    inverted: true,
                    shape: SelectionShape::Rect,
                },
            ),
        ],
    ));
    set.select(mask_selection(r(-7, 2, 33, 17), 3, true));
    set
}

fn same(a: &SelectionSet, b: &SelectionSet) {
    assert_eq!(a.active(), b.active());
    assert_eq!(a.saved_sorted(), b.saved_sorted());
}

fn container(selection: Option<&SelectionSet>) -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write_with_import_losses(
        &mut out,
        &tree(),
        &History::new(),
        (4, 4),
        None,
        &SkippedTiles::new(),
        None,
        selection,
        &mut store,
    ));
    out.into_inner()
}

fn open(bytes: &[u8]) -> Result<AurDocument, crate::IoError> {
    let (_dir, mut store) = store();
    read(Cursor::new(bytes), &mut store)
}

fn entry_names(bytes: &[u8]) -> Vec<String> {
    let archive = ok(zip::ZipArchive::new(Cursor::new(bytes)));
    archive.file_names().map(str::to_owned).collect()
}

/// `bytes` with the selection entry replaced by `payload` (or removed).
fn with_entry(bytes: &[u8], name: &str, payload: Option<&[u8]>) -> Vec<u8> {
    let mut archive = ok(zip::ZipArchive::new(Cursor::new(bytes)));
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let file = ok(archive.by_index_raw(index));
        if file.name() == name {
            continue;
        }
        ok(out.raw_copy_file(file));
    }
    if let Some(payload) = payload {
        ok(out.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        ));
        ok(std::io::Write::write_all(&mut out, payload));
    }
    ok(out.finish()).into_inner()
}

fn meta(kind: u8, bounds: Rect) -> SelectionMeta {
    SelectionMeta {
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
        inverted: false,
        kind,
    }
}

fn header_bytes(
    version: u32,
    active: Option<SelectionMeta>,
    saved: Vec<(String, SelectionMeta)>,
) -> Vec<u8> {
    ok(postcard::to_allocvec(&SelectionHeader {
        version,
        active,
        saved,
    }))
}

fn dropped(bytes: &[u8]) -> String {
    match decode_selection(bytes) {
        Ok(SelectionRead::Dropped(why)) => why,
        other => unreachable!("expected a drop, got {other:?}"),
    }
}

#[test]
fn every_shape_round_trips_bit_exactly_through_a_save() {
    let set = rich_set();
    let bytes = container(Some(&set));
    assert!(entry_names(&bytes).iter().any(|n| n == SELECTION_ENTRY));
    let document = ok(open(&bytes));
    assert_eq!(document.selection_dropped, None);
    same(&document.selection, &set);
    // The inverted mask kept its inverted flag, the ellipse its shape.
    let active = document.selection.active();
    assert!(active.as_ref().is_some_and(|s| s.inverted));
    let oval = document.selection.saved("Oval");
    assert!(oval.is_some_and(|s| s.inverted && matches!(s.shape, SelectionShape::Ellipse)));
}

#[test]
fn a_document_without_selections_writes_no_entry_and_the_same_bytes_as_before() {
    let none = container(None);
    let empty = container(Some(&SelectionSet::new()));
    assert!(!entry_names(&none).iter().any(|n| n == SELECTION_ENTRY));
    assert_eq!(none, empty, "an empty set writes nothing");
    let document = ok(open(&none));
    assert!(document.selection.is_empty());
    assert_eq!(document.selection_dropped, None);
}

#[test]
fn an_old_file_and_an_unknown_extra_entry_both_open() {
    let with = container(Some(&rich_set()));
    // What an older Aurora sees: the entry is never looked up. Removing it
    // is the same as that reader's view of the file, and an extra unknown
    // entry beside the known ones changes nothing.
    let stripped = with_entry(&with, SELECTION_ENTRY, None);
    assert!(ok(open(&stripped)).selection.is_empty());
    let extra = with_entry(&with, "selection-from-the-future", Some(b"whatever"));
    same(&ok(open(&extra)).selection, &rich_set());
}

#[test]
fn the_bytes_do_not_depend_on_hash_order() {
    let set = rich_set();
    let a = encode_selection(&set);
    let b = encode_selection(&set.clone());
    assert_eq!(ok(a), ok(b));
    let reordered = ok(SelectionSet::from_parts(
        set.active(),
        set.saved_sorted()
            .into_iter()
            .rev()
            .map(|(n, s)| (n.to_owned(), s.clone()))
            .collect(),
    ));
    assert_eq!(
        ok(encode_selection(&set)),
        ok(encode_selection(&reordered)),
        "names are written sorted"
    );
}

#[test]
fn a_newer_version_refuses_the_open_but_other_wrong_versions_only_drop_the_selection() {
    let bytes = container(None);
    let newer = header_bytes(SELECTION_VERSION + 1, None, Vec::new());
    let refused = with_entry(&bytes, SELECTION_ENTRY, Some(&newer));
    match open(&refused) {
        Err(crate::IoError::ManifestDeserialization(message)) => {
            assert!(message.contains("newer Aurora"), "{message}");
        }
        other => unreachable!("expected a refusal, got {:?}", other.map(|_| ())),
    }
    let old = header_bytes(0, None, Vec::new());
    let file = with_entry(&bytes, SELECTION_ENTRY, Some(&old));
    let document = ok(open(&file));
    assert!(document.selection.is_empty());
    assert!(document.selection_dropped.is_some(), "version 0");
}

#[test]
fn a_damaged_entry_drops_the_selection_and_never_the_file() {
    let bytes = container(Some(&rich_set()));
    let good = ok(encode_selection(&rich_set())).unwrap_or_default();
    let mut truncated = good.clone();
    truncated.truncate(good.len() - 5);
    let mut padded = good.clone();
    padded.push(0);
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![0xFF; 9],
        good.get(..6).unwrap_or_default().to_vec(),
        truncated,
        padded,
    ];
    for payload in cases {
        let file = with_entry(&bytes, SELECTION_ENTRY, Some(&payload));
        let document = ok(open(&file));
        assert!(document.selection.is_empty());
        assert!(document.selection_dropped.is_some());
        assert_eq!(document.layers.len(), 1, "the layers still open");
    }
}

#[test]
fn caps_are_checked_from_the_header_before_any_coverage_is_read() {
    // A header claiming a 64 Mpx + 1 mask, with no coverage behind it: it
    // is refused on the claim, so nothing is allocated for it.
    let too_big = header_bytes(
        SELECTION_VERSION,
        Some(meta(KIND_MASK, r(0, 0, 8193, 8192))),
        Vec::new(),
    );
    assert!(dropped(&too_big).contains("64 megapixels"));
    // Exactly 64 Mpx is within the cap (the size then does not add up).
    let at_cap = header_bytes(
        SELECTION_VERSION,
        Some(meta(KIND_MASK, r(0, 0, 8192, 8192))),
        Vec::new(),
    );
    assert!(dropped(&at_cap).contains("does not add up"));
    // Three saved masks of 50 MiB together pass the 128 MiB saved cap.
    let saved = |n: u32| {
        (0..n)
            .map(|i| (format!("m{i}"), meta(KIND_MASK, r(0, 0, 8192, 6400))))
            .collect::<Vec<_>>()
    };
    assert!(dropped(&header_bytes(SELECTION_VERSION, None, saved(3))).contains("too large"));
    // 64 saved names are within the count cap, 65 are not.
    let rects = |n: usize| {
        (0..n)
            .map(|i| (format!("s{i}"), meta(KIND_RECT, r(0, 0, 2, 2))))
            .collect::<Vec<_>>()
    };
    match ok(decode_selection(&header_bytes(
        SELECTION_VERSION,
        None,
        rects(64),
    ))) {
        SelectionRead::Restored(set) => assert_eq!(set.saved_count(), 64),
        other => unreachable!("{other:?}"),
    }
    assert!(
        dropped(&header_bytes(SELECTION_VERSION, None, rects(65))).contains("too many"),
        "65 saved selections"
    );
}

#[test]
fn bad_names_unknown_kinds_and_absurd_origins_drop_the_selection() {
    let one = |name: &str, m: SelectionMeta| {
        header_bytes(SELECTION_VERSION, None, vec![(name.to_owned(), m)])
    };
    let rect = meta(KIND_RECT, r(0, 0, 2, 2));
    for name in [
        "",
        " padded ",
        "tab\there",
        "bidi\u{202e}x",
        &"x".repeat(65),
    ] {
        assert!(!dropped(&one(name, rect)).is_empty(), "{name:?}");
    }
    assert!(!dropped(&one("ok", meta(7, r(0, 0, 2, 2)))).is_empty());
    assert!(!dropped(&one("ok", meta(KIND_ELLIPSE, r(i64::MAX - 1, 0, 9, 9)))).is_empty());
    let duplicate = header_bytes(
        SELECTION_VERSION,
        None,
        vec![("a".to_owned(), rect), ("a".to_owned(), rect)],
    );
    assert!(!dropped(&duplicate).is_empty());
    // A name of 64 multi-byte characters is fine.
    let long = "é".repeat(64);
    assert!(matches!(
        ok(decode_selection(&one(&long, rect))),
        SelectionRead::Restored(_)
    ));
}

#[test]
fn an_entry_past_the_read_cap_is_dropped_not_read() {
    let big = vec![0_u8; usize::try_from(super::MAX_SELECTION_ENTRY_BYTES + 1).unwrap_or(0)];
    let file = with_entry(&container(None), SELECTION_ENTRY, Some(&big));
    let document = ok(open(&file));
    assert!(document.selection.is_empty());
    let why = document.selection_dropped.unwrap_or_default();
    assert!(why.contains("larger than allowed"), "{why}");
}

#[test]
fn the_autosave_snapshot_carries_the_selection_and_counts_its_bytes() {
    let set = rich_set();
    let (_dir, mut store) = store();
    let layers = tree();
    let snapshot = ok(snapshot_best_effort(
        &layers,
        &History::new(),
        (4, 4),
        None,
        &SkippedTiles::new(),
        usize::MAX,
        &mut store,
    ));
    let Some(mut snapshot) = snapshot else {
        unreachable!("within budget")
    };
    assert_eq!(snapshot.selection_bytes(), 0);
    snapshot.set_selection(Some(&set));
    // The active 33 x 17 mask plus the saved "Hair" 9 x 9 one.
    assert_eq!(snapshot.selection_bytes(), 33 * 17 + 9 * 9);
    assert_eq!(
        snapshot.selection_bytes(),
        set.active().map_or(0, |s| s.mask_bytes()) + set.saved_mask_bytes()
    );
    let mut out = Cursor::new(Vec::new());
    ok(write_snapshot(&mut out, &snapshot));
    let document = ok(open(&out.into_inner()));
    same(&document.selection, &set);
    // Cleared again: no entry.
    snapshot.set_selection(None);
    let mut plain = Cursor::new(Vec::new());
    ok(write_snapshot(&mut plain, &snapshot));
    assert!(
        !entry_names(&plain.into_inner())
            .iter()
            .any(|n| n == SELECTION_ENTRY)
    );
}

#[test]
fn a_large_sparse_mask_is_small_in_the_file() {
    let bounds = r(0, 0, 4096, 4096);
    let mut data = vec![0_u8; 4096 * 4096];
    for byte in data.iter_mut().take(4096 * 100) {
        *byte = 255;
    }
    let selection = Selection::from_mask(Arc::new(ok(SelectionMask::new(bounds, data))));
    let mut set = SelectionSet::new();
    set.select(selection);
    let bytes = container(Some(&set));
    assert!(bytes.len() < 200_000, "deflated: {} bytes", bytes.len());
    same(&ok(open(&bytes)).selection, &set);
}
