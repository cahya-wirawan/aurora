//! Tests of the `paths` entry (0.233.0): round trips, no entry when there
//! is no path, old files, a newer version, damage and caps, and the
//! autosave snapshot.

use std::io::Cursor;

use aurora_core::Rect;
use aurora_doc::{
    Anchor, History, LayerTree, MAX_PATH_ANCHORS_TOTAL, Path, PathSet, SelectionSet, Subpath,
    WORK_PATH_NAME,
};
use aurora_tile::TileStore;

use super::{
    MAX_PATHS_ENTRY_BYTES, PATHS_ENTRY, PATHS_VERSION, PathDto, PathsHeader, PathsRead,
    decode_paths, encode_paths,
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

fn tree() -> LayerTree {
    let mut tree = LayerTree::new();
    ok(tree.add_pixel_layer(
        "A",
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        },
        None,
    ));
    tree
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
                    Anchor {
                        point: (-3.0, 40.0),
                        handle_in: (1.0, 2.0),
                        handle_out: (0.0, 0.0),
                    },
                ],
            },
            Subpath {
                closed: false,
                anchors: vec![Anchor::corner(0.0, 0.0), Anchor::corner(1.0, 1.0)],
            },
        ],
    }
}

fn sample_set() -> PathSet {
    ok(PathSet::from_parts(
        vec![sample_path("Outline", 1.0), sample_path("Zeta", 7.5)],
        Some(sample_path(WORK_PATH_NAME, 2.0)),
    ))
}

fn container(paths: Option<PathSet>) -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut set = SelectionSet::new();
    if let Some(paths) = paths {
        set.set_paths(paths);
    }
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write_with_import_losses(
        &mut out,
        &tree(),
        &History::new(),
        (4, 4),
        None,
        &SkippedTiles::new(),
        None,
        Some(&set),
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

fn dropped(bytes: &[u8]) -> String {
    match decode_paths(bytes) {
        Ok(PathsRead::Dropped(why)) => why,
        other => unreachable!("expected a drop, got {other:?}"),
    }
}

fn header(version: u32, paths: Vec<PathDto>, work: Option<PathDto>) -> Vec<u8> {
    ok(postcard::to_allocvec(&PathsHeader {
        version,
        paths,
        work,
    }))
}

fn dto(name: &str, anchors: usize) -> PathDto {
    PathDto {
        name: name.to_owned(),
        subpaths: vec![(true, vec![[1.0; 6]; anchors])],
    }
}

#[test]
fn paths_and_the_work_path_round_trip_bit_exactly_in_order() {
    let set = sample_set();
    let bytes = container(Some(set.clone()));
    assert!(entry_names(&bytes).iter().any(|n| n == PATHS_ENTRY));
    let document = ok(open(&bytes));
    assert_eq!(document.selection_dropped, None);
    assert_eq!(document.selection.paths(), &set);
    assert!(document.selection.is_empty(), "no selection appeared");
    assert_eq!(
        document.selection.paths().names().collect::<Vec<_>>(),
        vec!["Outline", "Zeta", WORK_PATH_NAME]
    );
}

#[test]
fn a_document_without_paths_writes_no_entry_and_the_same_bytes() {
    let none = container(None);
    let empty = container(Some(PathSet::new()));
    assert!(!entry_names(&none).iter().any(|n| n == PATHS_ENTRY));
    assert_eq!(none, empty);
    assert!(ok(open(&none)).selection.paths().is_empty());
}

#[test]
fn an_older_reader_and_an_unknown_extra_entry_both_open() {
    let with = container(Some(sample_set()));
    let stripped = with_entry(&with, PATHS_ENTRY, None);
    let document = ok(open(&stripped));
    assert!(document.selection.paths().is_empty());
    assert_eq!(document.selection_dropped, None);
    let extra = with_entry(&with, "future-thing", Some(b"whatever"));
    assert_eq!(ok(open(&extra)).selection.paths(), &sample_set());
}

#[test]
fn a_newer_version_refuses_the_open_and_an_older_one_is_dropped() {
    let newer = header(PATHS_VERSION + 1, vec![], None);
    let file = with_entry(&container(None), PATHS_ENTRY, Some(&newer));
    assert!(open(&file).is_err(), "a newer version refuses");
    let zero = header(0, vec![dto("a", 1)], None);
    assert!(!dropped(&zero).is_empty());
    let file = with_entry(&container(None), PATHS_ENTRY, Some(&zero));
    let document = ok(open(&file));
    assert!(document.selection.paths().is_empty());
    assert!(document.selection_dropped.is_some());
}

#[test]
fn damage_drops_the_paths_and_never_the_file() {
    let good = ok(encode_paths(&sample_set())).unwrap_or_default();
    assert!(!good.is_empty());
    for cut in [0, 1, 3, good.len() / 2, good.len() - 1] {
        let part = good.get(..cut).unwrap_or_default();
        let why = dropped(part);
        assert!(why.contains("paths"), "{why}");
    }
    let file = with_entry(
        &container(None),
        PATHS_ENTRY,
        Some(b"\xff\xff\xff\xff\xff\xff"),
    );
    let document = ok(open(&file));
    assert!(document.selection.paths().is_empty());
    assert!(document.selection_dropped.is_some());
    // The layers survived.
    assert_eq!(document.layers.len(), tree().len());
}

#[test]
fn caps_and_names_are_checked_when_reading() {
    let v = PATHS_VERSION;
    // Bad names.
    assert!(!dropped(&header(v, vec![dto("", 1)], None)).is_empty());
    assert!(!dropped(&header(v, vec![dto("a", 1), dto("a", 1)], None)).is_empty());
    assert!(!dropped(&header(v, vec![dto(WORK_PATH_NAME, 1)], None)).is_empty());
    assert!(!dropped(&header(v, vec![], Some(dto("Other", 1)))).is_empty());
    // A coordinate that is not finite.
    let mut nan = dto("n", 1);
    nan.subpaths = vec![(true, vec![[f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0]])];
    assert!(!dropped(&header(v, vec![nan], None)).is_empty());
    // Too many paths.
    let many: Vec<PathDto> = (0..65).map(|i| dto(&format!("p{i}"), 1)).collect();
    assert!(!dropped(&header(v, many, None)).is_empty());
    // Too many anchors in the document.
    let per = 10_000;
    let n = MAX_PATH_ANCHORS_TOTAL / per + 1;
    let heavy: Vec<PathDto> = (0..n).map(|i| dto(&format!("p{i}"), per)).collect();
    assert!(!dropped(&header(v, heavy, None)).is_empty());
    // The limit itself restores.
    let fits: Vec<PathDto> = (0..MAX_PATH_ANCHORS_TOTAL / per)
        .map(|i| dto(&format!("p{i}"), per))
        .collect();
    assert!(matches!(
        ok(decode_paths(&header(v, fits, None))),
        PathsRead::Restored(_)
    ));
}

#[test]
fn an_entry_past_the_read_cap_is_dropped_not_read() {
    let big = vec![0_u8; usize::try_from(MAX_PATHS_ENTRY_BYTES + 1).unwrap_or(0)];
    let file = with_entry(&container(None), PATHS_ENTRY, Some(&big));
    let document = ok(open(&file));
    assert!(document.selection.paths().is_empty());
    let why = document.selection_dropped.unwrap_or_default();
    assert!(why.contains("larger than allowed"), "{why}");
}

#[test]
fn the_autosave_snapshot_carries_the_paths() {
    let mut set = SelectionSet::new();
    set.set_paths(sample_set());
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
    snapshot.set_selection(Some(&set));
    let mut out = Cursor::new(Vec::new());
    ok(write_snapshot(&mut out, &snapshot));
    let document = ok(open(&out.into_inner()));
    assert_eq!(document.selection.paths(), &sample_set());
    snapshot.set_selection(None);
    let mut plain = Cursor::new(Vec::new());
    ok(write_snapshot(&mut plain, &snapshot));
    assert!(
        !entry_names(&plain.into_inner())
            .iter()
            .any(|n| n == PATHS_ENTRY)
    );
}

#[test]
fn paths_and_a_damaged_selection_report_both_reasons() {
    let file = with_entry(
        &container(Some(sample_set())),
        crate::aur_selection::SELECTION_ENTRY,
        Some(b"\xff\xff\xff\xff"),
    );
    let document = ok(open(&file));
    assert!(document.selection_dropped.is_some());
    assert_eq!(
        document.selection.paths(),
        &sample_set(),
        "paths still restored"
    );
}
