//! The file-access rules on real directories.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use aurora_mcp::{PathError, Roots};

fn roots(read: &std::path::Path, write: &std::path::Path) -> Roots {
    Roots::open(&[read.to_path_buf()], &[write.to_path_buf()]).unwrap()
}

#[test]
fn nothing_is_granted_by_default() {
    let roots = Roots::none();
    assert_eq!(roots.open_read("a", 10).unwrap_err(), PathError::NoReadRoot);
    assert_eq!(
        roots.check_write("a.png").unwrap_err(),
        PathError::NoWriteRoot
    );
    assert_eq!(
        roots.create_write("a.png").unwrap_err(),
        PathError::NoWriteRoot
    );
    assert!(!roots.can_read() && !roots.can_write());
}

#[test]
fn parent_components_are_refused_before_the_file_system() {
    let d = tempfile::tempdir().unwrap();
    let roots = roots(d.path(), d.path());
    for p in ["..", "../x", "a/../b", "a/..", "./../x"] {
        assert_eq!(
            roots.open_read(p, 10).unwrap_err(),
            PathError::ParentComponent,
            "{p}"
        );
        assert_eq!(
            roots.check_write(p).unwrap_err(),
            PathError::ParentComponent,
            "{p}"
        );
    }
}

#[test]
fn an_absolute_path_must_lie_inside_the_root() {
    let d = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f"), b"hi").unwrap();
    std::fs::write(other.path().join("f"), b"no").unwrap();
    let roots = roots(d.path(), d.path());
    let inside = d.path().canonicalize().unwrap().join("f");
    assert_eq!(
        roots.open_read(inside.to_str().unwrap(), 10).unwrap().len,
        2
    );
    let outside = other.path().join("f");
    assert!(matches!(
        roots.open_read(outside.to_str().unwrap(), 10).unwrap_err(),
        PathError::Outside(_)
    ));
    // a sibling whose name merely starts with the root's name
    let sibling = format!("{}x/f", d.path().display());
    assert!(matches!(
        roots.open_read(&sibling, 10).unwrap_err(),
        PathError::Outside(_)
    ));
}

#[test]
fn text_checks() {
    let d = tempfile::tempdir().unwrap();
    let roots = roots(d.path(), d.path());
    assert_eq!(roots.open_read("", 10).unwrap_err(), PathError::Empty);
    assert_eq!(
        roots.open_read("a\u{0}", 10).unwrap_err(),
        PathError::ControlCharacter
    );
    assert_eq!(
        roots.open_read(&"a".repeat(5000), 10).unwrap_err(),
        PathError::TooLong
    );
    assert_eq!(roots.open_read(".", 10).unwrap_err(), PathError::NoFileName);
}

#[cfg(unix)]
#[test]
fn symlinks_may_not_lead_out_of_a_root() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("s"), b"secret").unwrap();
    symlink(out.path().join("s"), d.path().join("file_link")).unwrap();
    symlink(out.path(), d.path().join("dir_link")).unwrap();
    symlink("../../..", d.path().join("up")).unwrap();
    let roots = roots(d.path(), d.path());
    assert_eq!(
        roots.open_read("file_link", 100).unwrap_err(),
        PathError::Escapes
    );
    assert_eq!(
        roots.open_read("dir_link/s", 100).unwrap_err(),
        PathError::Escapes
    );
    assert_eq!(
        roots.open_read("up/etc/passwd", 100).unwrap_err(),
        PathError::Escapes
    );
    assert_eq!(
        roots.create_write("dir_link/new").unwrap_err(),
        PathError::Escapes
    );
    assert!(!out.path().join("new").exists());
}

#[test]
fn create_write_is_exclusive_and_cleans_up_when_dropped() {
    let d = tempfile::tempdir().unwrap();
    let roots = roots(d.path(), d.path());
    let mut target = roots.create_write("new.bin").unwrap();
    assert!(target.file().is_some());
    assert!(matches!(
        roots.create_write("new.bin").unwrap_err(),
        PathError::Exists(_)
    ));
    assert!(matches!(
        roots.check_write("new.bin").unwrap_err(),
        PathError::Exists(_)
    ));
    drop(target);
    assert!(
        !d.path().join("new.bin").exists(),
        "an uncommitted file is removed"
    );
    let target = roots.create_write("kept.bin").unwrap();
    target.commit();
    assert!(d.path().join("kept.bin").exists());
}

#[test]
fn only_regular_files_within_the_limit_open() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    std::fs::write(d.path().join("sub/f"), b"12345").unwrap();
    let roots = roots(d.path(), d.path());
    assert_eq!(roots.open_read("sub", 10).unwrap_err(), PathError::NotAFile);
    assert!(matches!(
        roots.open_read("sub/f", 4).unwrap_err(),
        PathError::TooLarge { size: 5, max: 4 }
    ));
    assert!(matches!(
        roots.open_read("sub/none", 10).unwrap_err(),
        PathError::NotFound(_)
    ));
    assert_eq!(roots.open_read("sub/f", 5).unwrap().name, "f");
    assert!(matches!(
        roots.create_write("nodir/x").unwrap_err(),
        PathError::NoFolder(_)
    ));
}
