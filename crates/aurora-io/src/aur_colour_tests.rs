//! Tests of the `colour` entry: round trip, no entry when empty, old files,
//! a newer version, damage and caps, and the autosave snapshot.

use std::io::Cursor;

use aurora_color::IccProfile;
use aurora_core::Rect;
use aurora_doc::{History, LayerTree};
use aurora_tile::TileStore;

use super::{COLOUR_ENTRY, COLOUR_VERSION, ColourRead, DocumentColour, Wire, decode_colour};
use crate::aur::{AurDocument, SkippedTiles, read, snapshot_best_effort, write_snapshot};
use crate::colour_import::SourceProfile;

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

fn adobe() -> SourceProfile {
    let p = ok(IccProfile::synthetic_rgb(
        [0.3127, 0.3290],
        [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
        2.199_218_75,
    ));
    let bytes = ok(p.to_bytes());
    SourceProfile {
        description: ok(aurora_color::inspect_rgb_source(&bytes).map_err(|e| e.describe()))
            .description,
        bytes,
    }
}

fn container(colour: Option<&DocumentColour>) -> Vec<u8> {
    let (_dir, mut store) = store();
    let mut out = Cursor::new(Vec::new());
    ok(crate::aur::write_with_import_losses_and_colour(
        &mut out,
        &tree(),
        &History::new(),
        (4, 4),
        None,
        &SkippedTiles::new(),
        None,
        None,
        colour,
        &mut store,
    ));
    out.into_inner()
}

fn open(bytes: &[u8]) -> Result<AurDocument, crate::IoError> {
    let (_dir, mut store) = store();
    read(Cursor::new(bytes), &mut store)
}

fn names(bytes: &[u8]) -> Vec<String> {
    let archive = ok(zip::ZipArchive::new(Cursor::new(bytes)));
    archive
        .file_names()
        .filter_map(Result::ok)
        .map(std::borrow::Cow::into_owned)
        .collect()
}

fn with_entry(bytes: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut archive = ok(zip::ZipArchive::new(Cursor::new(bytes)));
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let file = ok(archive.by_index_raw(index));
        if file.name_raw() == COLOUR_ENTRY.as_bytes() {
            continue;
        }
        ok(out.raw_copy_file(file));
    }
    ok(out.start_file(COLOUR_ENTRY, zip::write::SimpleFileOptions::default()));
    ok(std::io::Write::write_all(&mut out, payload));
    ok(out.finish()).into_inner()
}

#[test]
fn source_and_output_profiles_round_trip() {
    let colour = DocumentColour {
        source: Some(adobe()),
        output: Some(adobe()),
    };
    let doc = ok(open(&container(Some(&colour))));
    assert_eq!(doc.colour, colour);
    assert_eq!(doc.colour_dropped, None);
    let only_out = DocumentColour {
        source: None,
        output: Some(adobe()),
    };
    assert_eq!(ok(open(&container(Some(&only_out)))).colour, only_out);
}

#[test]
fn a_document_without_colour_writes_no_entry_and_reads_as_empty() {
    for bytes in [container(None), container(Some(&DocumentColour::default()))] {
        assert!(!names(&bytes).iter().any(|n| n == COLOUR_ENTRY));
        let doc = ok(open(&bytes));
        assert!(doc.colour.is_empty());
        assert_eq!(doc.colour_dropped, None);
    }
    // And an entry in a file is the only difference.
    assert!(
        names(&container(Some(&DocumentColour {
            source: Some(adobe()),
            output: None
        })))
        .iter()
        .any(|n| n == COLOUR_ENTRY)
    );
}

#[test]
fn a_newer_version_refuses_and_damage_drops_without_failing_the_open() {
    let base = container(None);
    let newer = ok(postcard::to_allocvec(&Wire {
        version: COLOUR_VERSION + 1,
        source: None,
        output: None,
    }));
    assert!(open(&with_entry(&base, &newer)).is_err());
    for payload in [
        vec![0xFF_u8; 7],
        ok(postcard::to_allocvec(&Wire {
            version: COLOUR_VERSION,
            source: Some(b"not a profile".to_vec()),
            output: None,
        })),
    ] {
        let doc = ok(open(&with_entry(&base, &payload)));
        assert!(doc.colour.is_empty());
        assert!(doc.colour_dropped.is_some());
    }
    // An oversized entry is refused before decoding.
    let big = vec![0_u8; usize::try_from(super::MAX_COLOUR_ENTRY_BYTES).unwrap_or(0) + 1];
    let doc = ok(open(&with_entry(&base, &big)));
    assert!(doc.colour.is_empty() && doc.colour_dropped.is_some());
    assert!(matches!(decode_colour(&[]), Ok(ColourRead::Dropped(_))));
}

#[test]
fn the_autosave_snapshot_carries_the_colour() {
    let (_dir, mut store) = store();
    let Some(mut snapshot) = ok(snapshot_best_effort(
        &tree(),
        &History::new(),
        (4, 4),
        None,
        &SkippedTiles::new(),
        usize::MAX,
        &mut store,
    )) else {
        unreachable!("a snapshot")
    };
    let colour = DocumentColour {
        source: Some(adobe()),
        output: None,
    };
    ok(snapshot.set_colour(Some(&colour)));
    let mut out = Cursor::new(Vec::new());
    ok(write_snapshot(&mut out, &snapshot));
    assert_eq!(ok(open(&out.into_inner())).colour, colour);
}

#[test]
fn a_bad_source_profile_does_not_drop_a_good_output_profile() {
    let good = adobe();
    let base = container(None);
    let payload = ok(postcard::to_allocvec(&Wire {
        version: COLOUR_VERSION,
        source: Some(b"not a profile".to_vec()),
        output: Some(good.bytes.clone()),
    }));
    let doc = ok(open(&with_entry(&base, &payload)));
    assert_eq!(doc.colour.source, None);
    assert_eq!(doc.colour.output, Some(good.clone()));
    assert!(
        doc.colour_dropped
            .as_deref()
            .is_some_and(|w| w.contains("source"))
    );
    // And the other way round.
    let payload = ok(postcard::to_allocvec(&Wire {
        version: COLOUR_VERSION,
        source: Some(good.bytes.clone()),
        output: Some(vec![1, 2, 3]),
    }));
    let doc = ok(open(&with_entry(&base, &payload)));
    assert_eq!(doc.colour.source, Some(good));
    assert_eq!(doc.colour.output, None);
    assert!(
        doc.colour_dropped
            .as_deref()
            .is_some_and(|w| w.contains("output"))
    );
}
