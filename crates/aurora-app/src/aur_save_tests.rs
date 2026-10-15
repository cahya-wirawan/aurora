//! Tests of the safe `.aur` save (0.246.0, G-15) and of the canonical
//! manifest bytes it relies on (G-17). Everything runs through the free
//! helpers with an injected writer or verifier, so no window is needed.

use std::cell::RefCell;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use aurora_io::IoError;

use super::{AurSaveSource, replace_aur_verified, save_aur_document_with};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

/// The names (not paths) in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = ok(std::fs::read_dir(dir))
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

fn write_bytes(bytes: &'static [u8]) -> impl FnOnce(&mut std::fs::File) -> Result<(), IoError> {
    move |file| {
        file.write_all(bytes)?;
        Ok(())
    }
}

fn is_unique_hidden_temp_of(name: &str, target_name: &str) -> bool {
    name.starts_with(&format!(".{target_name}."))
        && name.rsplit('.').next() == Some("tmp")
        && name != target_name
}

/// G-15: the temp file is a fresh hidden, randomly named file, never the
/// old fixed `<name>.tmp`; it is the path the verifier is handed; it is
/// gone after success; and two saves do not share a temp name.
#[test]
fn the_temp_file_is_unique_hidden_verified_by_path_and_gone_afterwards() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.aur");
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    for round in 0..2 {
        let verified: RefCell<Option<PathBuf>> = RefCell::new(None);
        ok(replace_aur_verified(
            &target,
            |file| {
                // While writing, exactly one extra file exists: the temp.
                let during: Vec<String> = names(dir.path())
                    .into_iter()
                    .filter(|n| n != "doc.aur")
                    .collect();
                assert_eq!(during.len(), 1, "round {round}: {during:?}");
                let Some(temp_name) = during.first() else {
                    unreachable!("one temp");
                };
                assert!(is_unique_hidden_temp_of(temp_name, "doc.aur"), "{during:?}");
                assert_ne!(temp_name, "doc.aur.tmp", "the old fixed temp name");
                seen.borrow_mut().push(temp_name.clone());
                file.write_all(b"new contents")?;
                Ok(())
            },
            |temp| {
                *verified.borrow_mut() = Some(temp.to_path_buf());
                // Verification sees the data on disk, in the temp file.
                std::fs::read(temp).is_ok_and(|b| b == b"new contents")
            },
        ));
        let Some(path) = verified.into_inner() else {
            unreachable!("the verifier ran");
        };
        assert_eq!(path.parent(), Some(dir.path()), "beside the target");
        assert_eq!(
            names(dir.path()),
            vec!["doc.aur".to_owned()],
            "no temp left"
        );
        assert_eq!(ok(std::fs::read(&target)), b"new contents");
    }
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 2);
    assert_ne!(
        seen.first(),
        seen.get(1),
        "each save gets its own temp name"
    );
}

/// G-15: the user's own `<name>.tmp` (the name the old code truncated and
/// overwrote) is never touched.
#[test]
fn a_users_own_name_dot_tmp_file_is_left_alone() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.aur");
    let theirs = dir.path().join("doc.aur.tmp");
    ok(std::fs::write(&theirs, b"precious"));
    ok(replace_aur_verified(&target, write_bytes(b"saved"), |_| {
        true
    }));
    assert_eq!(ok(std::fs::read(&theirs)), b"precious");
    assert_eq!(ok(std::fs::read(&target)), b"saved");
}

/// G-15: an existing destination is not opened for writing: it holds its
/// old bytes through the write and the verify, and changes only at the
/// rename.
#[test]
fn an_existing_destination_is_never_truncated_before_the_rename() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.aur");
    ok(std::fs::write(&target, b"old contents"));
    ok(replace_aur_verified(
        &target,
        |file| {
            assert_eq!(ok(std::fs::read(&target)), b"old contents", "during write");
            file.write_all(b"new")?;
            Ok(())
        },
        |_| {
            assert_eq!(ok(std::fs::read(&target)), b"old contents", "during verify");
            true
        },
    ));
    assert_eq!(ok(std::fs::read(&target)), b"new");
}

/// G-15: a writer that fails part way leaves the destination intact and
/// no temp file behind, and the error is the writer's.
#[test]
fn a_failing_writer_keeps_the_destination_and_removes_the_temp() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.aur");
    ok(std::fs::write(&target, b"old contents"));
    let result = replace_aur_verified(
        &target,
        |file| {
            file.write_all(b"half of a docu")?;
            Err(IoError::Io(std::io::Error::other("disk on fire")))
        },
        |_| unreachable!("verification must not run after a failed write"),
    );
    assert!(matches!(result, Err(IoError::Io(_))), "{result:?}");
    assert_eq!(ok(std::fs::read(&target)), b"old contents");
    assert_eq!(names(dir.path()), vec!["doc.aur".to_owned()]);
}

/// G-15: a file that does not verify is not swapped in, and its temp is
/// removed. Also for a destination that does not exist yet.
#[test]
fn a_failing_verifier_keeps_the_destination_and_removes_the_temp() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.aur");
    ok(std::fs::write(&target, b"old contents"));
    let result = replace_aur_verified(&target, write_bytes(b"bad"), |_| false);
    match result {
        Err(IoError::Io(err)) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidData),
        other => unreachable!("{other:?}"),
    }
    assert_eq!(ok(std::fs::read(&target)), b"old contents");
    assert_eq!(names(dir.path()), vec!["doc.aur".to_owned()]);

    let fresh = dir.path().join("fresh.aur");
    assert!(replace_aur_verified(&fresh, write_bytes(b"bad"), |_| false).is_err());
    assert!(!fresh.exists(), "nothing was created");
    assert_eq!(names(dir.path()), vec!["doc.aur".to_owned()]);
}

/// G-15: saving to a symbolic link replaces the real file and keeps the
/// link (Unix: creating a link needs no privilege there).
#[cfg(unix)]
#[test]
fn a_symbolic_link_target_is_replaced_at_its_real_location() {
    let dir = ok(tempfile::tempdir());
    let real_dir = dir.path().join("real");
    ok(std::fs::create_dir(&real_dir));
    let real = real_dir.join("doc.aur");
    ok(std::fs::write(&real, b"old contents"));
    let link = dir.path().join("link.aur");
    ok(std::os::unix::fs::symlink(&real, &link));
    let temp_dir_seen: RefCell<Option<PathBuf>> = RefCell::new(None);
    ok(replace_aur_verified(&link, write_bytes(b"new"), |temp| {
        *temp_dir_seen.borrow_mut() = temp.parent().map(Path::to_path_buf);
        true
    }));
    let meta = ok(std::fs::symlink_metadata(&link));
    assert!(meta.file_type().is_symlink(), "the link stays a link");
    assert_eq!(ok(std::fs::read(&real)), b"new");
    assert_eq!(ok(std::fs::read(&link)), b"new");
    // The temp sat beside the real file (same file system), not the link.
    let seen = temp_dir_seen.into_inner();
    assert_eq!(
        seen.and_then(|p| std::fs::canonicalize(p).ok()),
        std::fs::canonicalize(&real_dir).ok()
    );
    assert_eq!(names(&real_dir), vec!["doc.aur".to_owned()]);
    assert_eq!(
        names(dir.path()),
        vec!["link.aur".to_owned(), "real".to_owned()]
    );
}

/// A throwaway store for the document tests.
fn store() -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero");
    };
    let store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    (dir, store)
}

/// A document of `n` pixel layers and a group, journalled.
fn document(n: u32) -> (aurora_doc::LayerTree, aurora_doc::History) {
    let (mut layers, mut history, _first) = crate::document_from_size("Background", (64, 48));
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 48,
    };
    for i in 1..n {
        ok(history.add_pixel_layer(&mut layers, format!("Layer {i}"), bounds, None));
    }
    (layers, history)
}

/// Reads the container's `manifest` entry and its entry-name list.
fn manifest_and_entries(file: &Path) -> (Vec<u8>, Vec<String>) {
    let mut archive = ok(zip::ZipArchive::new(Cursor::new(ok(std::fs::read(file)))));
    let entries: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let mut manifest = Vec::new();
    let mut entry = ok(archive.by_name("manifest"));
    ok(entry.read_to_end(&mut manifest));
    (manifest, entries)
}

/// Saves `layers`/`history` to `path` the way the app does, verified by a
/// real read-back into a throwaway store (not `verify_aur`, which shares
/// a scratch directory with another test).
fn save(path: &Path, layers: &aurora_doc::LayerTree, history: &aurora_doc::History) {
    let (_dir, mut live) = store();
    let selection = aurora_doc::SelectionSet::new();
    let skipped = aurora_io::SkippedTiles::new();
    let source = AurSaveSource {
        layers,
        history,
        canvas_size: (64, 48),
        skipped_tiles: &skipped,
        import_losses: None,
        selection: &selection,
    };
    ok(save_aur_document_with(path, &source, &mut live, |temp| {
        let (_scratch, mut check) = store();
        std::fs::File::open(temp)
            .ok()
            .is_some_and(|file| aurora_io::read_aur(file, &mut check).is_ok())
    }));
}

/// The saved file reopens to the same document, and a failed verification
/// of a real document leaves the previous file and no temp.
#[test]
fn the_saved_file_reopens_identically_and_a_failed_verify_changes_nothing() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("doc.aur");
    let (layers, history) = document(5);
    save(&path, &layers, &history);
    assert_eq!(names(dir.path()), vec!["doc.aur".to_owned()]);

    let (_scratch, mut check) = store();
    let reopened = ok(aurora_io::read_aur(
        ok(std::fs::File::open(&path)),
        &mut check,
    ));
    assert_eq!(reopened.canvas_size, (64, 48));
    assert_eq!(reopened.layers.len(), layers.len());
    let names_of = |tree: &aurora_doc::LayerTree| -> Vec<String> {
        tree.roots()
            .iter()
            .copied()
            .map(|id| tree.name(id).unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(names_of(&reopened.layers), names_of(&layers));

    // A second save whose verifier refuses: the first file is untouched.
    let before = ok(std::fs::read(&path));
    let (smaller, small_history) = document(2);
    let (_dir2, mut live) = store();
    let selection = aurora_doc::SelectionSet::new();
    let skipped = aurora_io::SkippedTiles::new();
    let source = AurSaveSource {
        layers: &smaller,
        history: &small_history,
        canvas_size: (64, 48),
        skipped_tiles: &skipped,
        import_losses: None,
        selection: &selection,
    };
    assert!(save_aur_document_with(&path, &source, &mut live, |_| false).is_err());
    assert_eq!(ok(std::fs::read(&path)), before);
    assert_eq!(names(dir.path()), vec!["doc.aur".to_owned()]);
}

/// G-17 end to end: saving the same document twice, and saving a document
/// that was loaded from a save (its layer map is rebuilt with a different
/// hash state each time), gives the same manifest bytes and the same
/// container entry list.
#[test]
fn saves_of_the_same_document_have_identical_manifest_bytes_and_entries() {
    let dir = ok(tempfile::tempdir());
    let (layers, history) = document(12);
    let first = dir.path().join("first.aur");
    save(&first, &layers, &history);
    let (manifest, entries) = manifest_and_entries(&first);

    let again = dir.path().join("again.aur");
    save(&again, &layers, &history);
    let (manifest_again, entries_again) = manifest_and_entries(&again);
    assert_eq!(manifest, manifest_again);
    assert_eq!(entries, entries_again);

    // Reload and re-save several times: every reload builds its own
    // `HashMap` (a fresh hash state), which used to reorder the manifest.
    let mut current = first;
    for round in 0..6 {
        let (_scratch, mut check) = store();
        let loaded = ok(aurora_io::read_aur(
            ok(std::fs::File::open(&current)),
            &mut check,
        ));
        let next = dir.path().join(format!("round{round}.aur"));
        save(&next, &loaded.layers, &loaded.history);
        let (m, e) = manifest_and_entries(&next);
        assert_eq!(m, manifest, "round {round}");
        assert_eq!(e, entries, "round {round}");
        current = next;
    }
}

/// G-15: `save_aur_file` goes through the safe path and keeps none of the
/// old fixed-temp code (a single-line check of the source, so it cannot
/// be defeated by line endings).
#[test]
fn save_aur_file_uses_the_safe_save_path() {
    let source = include_str!("lib.rs");
    let Some(start) = source.find("fn save_aur_file(") else {
        unreachable!("save_aur_file exists");
    };
    let body = source.get(start..).unwrap_or_default();
    let end = body
        .find("/// A real `WindowEvent::CursorMoved`")
        .map_or(body.len(), |n| n);
    let body = body.get(..end).unwrap_or_default();
    assert!(
        body.contains("aur_save::save_aur_document("),
        "wired to the helper"
    );
    assert!(!body.contains("File::create"), "no plain create");
    assert!(!body.contains("fs::rename"), "no bare rename");
    assert!(!body.contains("push(\".tmp\")"), "no fixed temp name");
}
