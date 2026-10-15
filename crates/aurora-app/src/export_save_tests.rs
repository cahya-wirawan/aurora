//! Tests of the safe flat-image export (0.247.0). Everything runs through
//! the free helpers with injected syncs, so no window is needed.

use std::cell::Cell;
use std::path::Path;

use super::{write_export_verified, write_export_verified_with};

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

fn image(width: u32, height: u32) -> aurora_io::Image {
    let samples: Vec<half::f16> = (0..width as usize * height as usize)
        .flat_map(|_| [0.0, 1.0, 0.0, 1.0].map(half::f16::from_f32))
        .collect();
    ok(aurora_io::Image::new(
        width,
        height,
        aurora_color::IccProfile::srgb(),
        samples,
    ))
}

fn encoded(path: &Path, width: u32, height: u32) -> Vec<u8> {
    ok(aurora_io::encode_by_extension(path, &image(width, height)))
}

fn assert_real_image(path: &Path, width: u32, height: u32) {
    let bytes = ok(std::fs::read(path));
    let decoded = ok(aurora_io::decode_by_extension(path, &bytes));
    assert_eq!((decoded.width(), decoded.height()), (width, height));
}

fn sync_ok(file: &std::fs::File) -> std::io::Result<()> {
    file.sync_all()
}

fn dir_ok(_: &Path) -> std::io::Result<()> {
    Ok(())
}

fn is_unique_hidden_temp_of(name: &str, target_name: &str) -> bool {
    name.starts_with(&format!(".{target_name}."))
        && name.rsplit('.').next() == Some("tmp")
        && name != format!("{target_name}.tmp")
}

#[test]
fn every_export_format_still_produces_a_valid_file_and_leaves_only_it() {
    for name in ["export.png", "export.jpg", "export.tif"] {
        let dir = ok(tempfile::tempdir());
        let path = dir.path().join(name);
        let bytes = encoded(&path, 5, 3);
        ok(write_export_verified(&path, &bytes, 5, 3));
        assert_real_image(&path, 5, 3);
        assert_eq!(names(dir.path()), [name], "{name}: only the export remains");
    }
}

#[test]
fn the_temp_file_is_unique_hidden_and_exclusive_while_it_exists_and_gone_after() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    let bytes = encoded(&path, 4, 4);
    let seen = Cell::new(false);
    ok(write_export_verified_with(
        &path,
        &bytes,
        4,
        4,
        |file| {
            let found = names(dir.path());
            assert_eq!(found.len(), 1, "exactly the temp file exists: {found:?}");
            assert!(
                is_unique_hidden_temp_of(&found[0], "export.png"),
                "not a hidden unique name: {found:?}"
            );
            seen.set(true);
            file.sync_all()
        },
        dir_ok,
    ));
    assert!(seen.get());
    assert_eq!(names(dir.path()), ["export.png"]);
}

#[test]
fn a_users_own_name_dot_tmp_is_never_touched_on_success_or_failure() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    let theirs = dir.path().join("export.png.tmp");
    ok(std::fs::write(&theirs, b"my notes"));
    let bytes = encoded(&path, 4, 4);

    ok(write_export_verified(&path, &bytes, 4, 4));
    assert_eq!(ok(std::fs::read(&theirs)), b"my notes");

    assert!(write_export_verified(&path, b"not a png", 4, 4).is_err());
    assert_eq!(ok(std::fs::read(&theirs)), b"my notes");
    assert_eq!(names(dir.path()), ["export.png", "export.png.tmp"]);
}

#[test]
fn corrupt_bytes_fail_leave_no_temp_and_create_no_destination() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    assert!(write_export_verified(&path, b"not a real png", 4, 4).is_err());
    assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
}

#[test]
fn a_failed_verify_leaves_an_existing_destination_byte_for_byte_intact() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    ok(std::fs::write(&path, b"pre-existing content"));
    assert!(write_export_verified(&path, b"not a real png", 4, 4).is_err());
    assert_eq!(ok(std::fs::read(&path)), b"pre-existing content");
    assert_eq!(names(dir.path()), ["export.png"]);
}

#[test]
fn a_size_mismatch_is_a_verify_failure() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    ok(std::fs::write(&path, b"old"));
    let bytes = encoded(&path, 4, 4);
    assert!(write_export_verified(&path, &bytes, 8, 8).is_err());
    assert_eq!(ok(std::fs::read(&path)), b"old");
    assert_eq!(names(dir.path()), ["export.png"]);
}

#[test]
fn the_destination_is_not_written_until_the_verified_temp_is_renamed() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    ok(std::fs::write(&path, b"old"));
    let bytes = encoded(&path, 4, 4);
    ok(write_export_verified_with(
        &path,
        &bytes,
        4,
        4,
        |file| {
            assert_eq!(
                ok(std::fs::read(&path)),
                b"old",
                "the destination must not be opened or truncated before the rename"
            );
            file.sync_all()
        },
        dir_ok,
    ));
    assert_real_image(&path, 4, 4);
}

#[test]
fn the_data_sync_runs_once_before_the_rename_and_its_failure_is_fatal() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    ok(std::fs::write(&path, b"old"));
    let bytes = encoded(&path, 4, 4);

    let syncs = Cell::new(0_u32);
    ok(write_export_verified_with(
        &path,
        &bytes,
        4,
        4,
        |file| {
            syncs.set(syncs.get() + 1);
            file.sync_all()
        },
        dir_ok,
    ));
    assert_eq!(
        syncs.get(),
        1,
        "the temp file's data is synced exactly once"
    );

    ok(std::fs::write(&path, b"old"));
    let result = write_export_verified_with(
        &path,
        &bytes,
        4,
        4,
        |_| Err(std::io::Error::other("disk full")),
        dir_ok,
    );
    assert!(result.is_err());
    assert_eq!(ok(std::fs::read(&path)), b"old");
    assert_eq!(
        names(dir.path()),
        ["export.png"],
        "the temp file is removed"
    );
}

#[test]
fn a_directory_sync_failure_is_a_success_with_an_advisory_and_the_file_is_in_place() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    let bytes = encoded(&path, 4, 4);
    let calls = Cell::new(0_u32);
    let advisory = ok(write_export_verified_with(
        &path,
        &bytes,
        4,
        4,
        sync_ok,
        |synced| {
            calls.set(calls.get() + 1);
            assert_eq!(synced, dir.path());
            Err(std::io::Error::other("this mount cannot sync a directory"))
        },
    ));
    assert_eq!(calls.get(), 1);
    assert_eq!(advisory.as_deref(), Some(aurora_io::DIR_SYNC_ADVISORY));
    assert_real_image(&path, 4, 4);
    assert_eq!(names(dir.path()), ["export.png"]);
}

#[test]
fn a_confirmed_directory_sync_gives_no_advisory() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("export.png");
    let bytes = encoded(&path, 4, 4);
    let advisory = ok(write_export_verified_with(
        &path, &bytes, 4, 4, sync_ok, dir_ok,
    ));
    assert_eq!(advisory, None);
}

#[test]
fn a_path_with_no_file_name_is_refused() {
    assert!(write_export_verified(Path::new("/"), b"x", 1, 1).is_err());
}

#[cfg(unix)]
#[test]
fn an_export_through_a_symbolic_link_replaces_the_real_file_and_keeps_the_link() {
    let dir = ok(tempfile::tempdir());
    let real_dir = dir.path().join("real");
    ok(std::fs::create_dir(&real_dir));
    let real = real_dir.join("photo.png");
    ok(std::fs::write(&real, b"old"));
    let link = dir.path().join("link.png");
    ok(std::os::unix::fs::symlink(&real, &link));

    let bytes = encoded(&link, 4, 4);
    ok(write_export_verified(&link, &bytes, 4, 4));

    let meta = ok(std::fs::symlink_metadata(&link));
    assert!(meta.file_type().is_symlink(), "the link must stay a link");
    assert_real_image(&real, 4, 4);
    assert_eq!(
        names(&real_dir),
        ["photo.png"],
        "the temp sat beside the real file"
    );
    assert_eq!(names(dir.path()), ["link.png", "real"]);
}

#[cfg(unix)]
#[test]
fn an_export_to_a_dangling_link_is_refused_and_leaves_nothing() {
    let dir = ok(tempfile::tempdir());
    let link = dir.path().join("link.png");
    ok(std::os::unix::fs::symlink(
        dir.path().join("missing.png"),
        &link,
    ));
    let bytes = encoded(&link, 4, 4);
    assert!(write_export_verified(&link, &bytes, 4, 4).is_err());
    assert_eq!(names(dir.path()), ["link.png"]);
    assert!(
        ok(std::fs::symlink_metadata(&link))
            .file_type()
            .is_symlink()
    );
}

#[test]
fn the_export_caller_goes_through_the_safe_path() {
    // A source-level pin (single-line needles only: CRLF-safe).
    let source = include_str!("lib.rs");
    assert!(source.contains("export_save::write_export_verified("));
    assert!(!source.contains("fn write_verified("));
}

#[test]
fn the_unconfirmed_save_notice_says_so_and_stays_while_its_tool_changes() {
    use crate::LockRefusal;
    assert_eq!(
        LockRefusal::SaveNotConfirmed.message(),
        aurora_io::DIR_SYNC_ADVISORY
    );
    assert!(aurora_io::DIR_SYNC_ADVISORY.contains("saved"));
    for tool in [aurora_ui::Tool::Move, aurora_ui::Tool::Brush] {
        assert!(crate::lock_refusal_applies(
            LockRefusal::SaveNotConfirmed,
            tool,
            aurora_doc::LayerLock::default()
        ));
    }
}

#[test]
fn a_psd_step_distinguishes_a_confirmed_save_from_an_unconfirmed_one() {
    use crate::psd_save::PsdSaveStep;
    assert!(!matches!(
        PsdSaveStep::SavedNotConfirmed,
        PsdSaveStep::Saved
    ));
}

#[test]
fn the_save_callers_surface_the_advisory() {
    // Source-level pins (single-line needles only: CRLF-safe).
    let source = include_str!("lib.rs");
    assert!(source.contains("self.note_save_advisory(advisory.as_deref());"));
    assert!(source.contains("self.note_save_advisory(Some(aurora_io::DIR_SYNC_ADVISORY));"));
}
