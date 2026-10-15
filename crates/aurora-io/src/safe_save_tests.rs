//! Tests of the injectable syncs in [`super`] (0.247.0): a directory-sync
//! failure after the rename is a success with an advisory, a data-sync
//! failure before it is an error. The real file system is used only for
//! the files; the syncs are closures.

use std::cell::Cell;
use std::io::Write as _;
use std::path::Path;

use super::{
    DIR_SYNC_ADVISORY, replace_verified, replace_verified_advised, write_verified_temp_with,
};
use crate::error::IoError;

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = ok(std::fs::read_dir(dir))
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

fn read(path: &Path) -> Vec<u8> {
    ok(std::fs::read(path))
}

fn put(bytes: &'static [u8]) -> impl FnOnce(&mut std::fs::File) -> Result<(), IoError> {
    move |file| {
        file.write_all(bytes)?;
        Ok(())
    }
}

#[allow(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]
fn accept(_: &Path, (): &()) -> Result<(), IoError> {
    Ok(())
}

fn sync_ok(file: &std::fs::File) -> std::io::Result<()> {
    file.sync_all()
}

#[test]
fn a_directory_sync_failure_after_the_rename_is_a_success_with_an_advisory() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.bin");
    ok(std::fs::write(&target, b"old"));
    let ((), temp) = ok(write_verified_temp_with(
        &target,
        put(b"new"),
        sync_ok,
        accept,
    ));
    let advisory = ok(temp.persist_with(|_| Err(std::io::Error::other("no dir sync here"))));
    assert_eq!(advisory.as_deref(), Some(DIR_SYNC_ADVISORY));
    assert_eq!(read(&target), b"new", "the new file is in place");
    assert_eq!(names(dir.path()), ["doc.bin"], "no temp file is left");
}

#[test]
fn a_confirmed_directory_sync_gives_no_advisory_and_runs_after_the_rename() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.bin");
    let ((), temp) = ok(write_verified_temp_with(
        &target,
        put(b"new"),
        sync_ok,
        accept,
    ));
    let calls = Cell::new(0_u32);
    let advisory = ok(temp.persist_with(|synced| {
        calls.set(calls.get() + 1);
        assert_eq!(synced, dir.path(), "the target's own directory is synced");
        assert_eq!(
            std::fs::read(&target)?,
            b"new",
            "the directory is synced after the rename, not before"
        );
        Ok(())
    }));
    assert_eq!(advisory, None);
    assert_eq!(calls.get(), 1);
}

#[test]
fn replace_verified_succeeds_when_the_real_directory_sync_would_be_the_only_failure() {
    // `replace_verified` uses the real sync; here it simply must keep
    // working and report no advisory on an ordinary file system.
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.bin");
    let ((), advisory) = ok(replace_verified_advised(&target, put(b"x"), accept));
    assert_eq!(advisory, None);
    ok(replace_verified(&target, put(b"y"), accept));
    assert_eq!(read(&target), b"y");
}

#[test]
fn a_data_sync_failure_is_fatal_and_leaves_no_temp_and_the_target_intact() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.bin");
    ok(std::fs::write(&target, b"old"));
    let result = write_verified_temp_with(
        &target,
        put(b"new"),
        |_| Err(std::io::Error::other("disk full")),
        accept,
    );
    assert!(result.is_err());
    assert_eq!(read(&target), b"old");
    assert_eq!(names(dir.path()), ["doc.bin"]);
}

#[test]
fn the_data_sync_runs_once_after_the_write_and_before_the_verify_and_the_rename() {
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.bin");
    ok(std::fs::write(&target, b"old"));
    let syncs = Cell::new(0_u32);
    let verified_after_sync = Cell::new(false);
    let ((), _temp) = ok(write_verified_temp_with(
        &target,
        put(b"new"),
        |file| {
            syncs.set(syncs.get() + 1);
            assert_eq!(
                std::fs::read(&target)?,
                b"old",
                "the target is not touched while the temp file is synced"
            );
            assert_eq!(file.metadata()?.len(), 3, "synced after the write");
            Ok(())
        },
        |_, ()| {
            verified_after_sync.set(syncs.get() == 1);
            Ok(())
        },
    ));
    assert_eq!(syncs.get(), 1);
    assert!(verified_after_sync.get());
}

#[test]
fn a_failed_rename_is_still_an_error_and_leaves_no_temp() {
    // A non-empty directory cannot be replaced by a file.
    let dir = ok(tempfile::tempdir());
    let target = dir.path().join("doc.bin");
    ok(std::fs::create_dir(&target));
    ok(std::fs::write(target.join("inner"), b"keep"));
    let outcome = write_verified_temp_with(&target, put(b"new"), sync_ok, accept);
    let result = match outcome {
        Ok(((), temp)) => temp.persist_with(|_| Ok(())),
        Err(err) => Err(err),
    };
    assert!(result.is_err());
    assert_eq!(read(&target.join("inner")), b"keep");
    assert_eq!(names(dir.path()), ["doc.bin"], "no temp file is left");
}
