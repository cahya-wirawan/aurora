//! Scratch-directory liveness: an advisory lock a running process holds
//! for its own scratch directory, and a startup sweep that removes the
//! directories of processes that are gone.
//!
//! **Why this exists.** A [`crate::TileStore`]'s scratch directory holds
//! this session's paged-out tiles — potentially gigabytes of a
//! professional's unsaved pixels. `aurora-app` removes its own on a
//! clean exit (0.63.0 extended that to every `winit` exit path), but a
//! hard crash, `SIGKILL`, an OS shutdown, and the release profile's
//! `panic = "abort"` all skip it, so leftovers accumulate in the system
//! temp directory forever. That was the half deliberately left open in
//! 0.63.0, because deleting another process's directory needs a
//! *liveness check* and getting one wrong is far worse than the leak.
//!
//! **The whole design is fail-closed.** A false "dead" verdict deletes a
//! live session's real unsaved pixels; a false "alive" verdict leaks a
//! directory someone can remove by hand. Those are not comparable, so
//! every branch that is not a positive proof of death — no lock file, a
//! lock already held, a permission error, an unreadable entry, a
//! metadata failure, anything at all — counts as `skipped` and deletes
//! nothing. There is no `unwrap_or(true)`-shaped path anywhere in here;
//! [`Verdict`] exists specifically so the control flow cannot grow one.
//!
//! **`flock`, not `fcntl`.** The lock attaches to the open file
//! *description*, which buys two things: the kernel releases it when the
//! process dies however it dies (this is the entire liveness signal),
//! and a second accidental lock attempt from *within this same process*
//! conflicts rather than silently succeeding. `fcntl`/`F_SETLK` locks
//! are per-process and are dropped by a `close(2)` on *any* descriptor
//! for the same file, anywhere in the process — a footgun this crate
//! does not need.
//!
//! ## Residual races and limits, stated plainly
//!
//! - **A directory created but not yet locked is skipped, not deleted.**
//!   There are a few microseconds between `mkdir` and the `flock` that
//!   follows it, and in that window the directory has no lock file at
//!   all — which this sweep reads as "unknown", never as "dead". This is
//!   why the order must stay *create the directory, then lock it*:
//!   reversing it would open a window where a live directory looks dead.
//!   **The lock file itself must therefore never exist unlocked**, which
//!   is a stronger claim than "the directory is created first" and is
//!   what [`lock_scratch_dir`] now actually guarantees: it creates a
//!   uniquely named temporary file, `flock`s *that*, and only then
//!   publishes it under [`LOCK_FILE_NAME`] with `link(2)`, which is
//!   atomic. Until 0.68.1 the sequence was `open(O_CREAT)` then `flock`,
//!   so `aurora.lock` really did exist unlocked for the duration of one
//!   syscall — long enough for a concurrent sweep to read it as `Dead`
//!   and `remove_dir_all` a directory whose owner was, at that instant,
//!   still taking its lock. That was a live-data-loss race, not a
//!   theoretical one; see this module's own
//!   `the_canonical_lock_file_is_never_visible_before_it_is_locked`.
//! - **Pre-0.67.0 leftovers are never swept.** They have no lock file,
//!   so they fall into the same "unknown" branch by design. Removing
//!   them is a manual job, once. This is deliberate: "no lock file"
//!   cannot mean "dead" without also meaning it for the race above.
//! - **`flock` is not reliable over NFS or other network filesystems.**
//!   Linux emulates it via `fcntl` byte-range locks over NFS, and other
//!   platforms vary. A scratch directory on a network mount is outside
//!   what this can promise — and is a bad idea for a tile store's paging
//!   path regardless.
//! - **Windows is not covered at all this round** — see the
//!   `#[cfg(not(unix))]` arms below, which are honest no-ops rather than
//!   a guess at an equivalent.

use std::path::Path;

/// What [`sweep_orphaned_scratch_dirs`] did.
///
/// `removed + skipped` is every entry whose name matched the prefix;
/// entries that did not match are not counted at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Directories proven dead — their lock was acquired, so no process
    /// held it — and therefore deleted.
    pub removed: usize,
    /// Matching entries left alone for **any** reason: a live lock, a
    /// missing lock file, a symlink, a plain file, a foreign owner, or
    /// any error at all along the way. Never a deletion.
    pub skipped: usize,
}

/// The name of the lock file inside each scratch directory.
///
/// It lives *inside* the directory rather than beside it as
/// `<dir>.lock`, deliberately: the scratch directory is created 0700 and
/// its ownership is verified before anything is opened inside it
/// ([`crate::TileStore`]'s own `create_private_dir`), so nothing another
/// user can create is reachable there. A sibling in a world-writable,
/// sticky `/tmp` has no such protection — an attacker can pre-create it
/// and the sweep would then be reasoning about a file it does not own.
///
/// It is not a `.tile` file and never will be, which is what keeps every
/// tile enumerator in this workspace correct in its presence.
pub const LOCK_FILE_NAME: &str = "aurora.lock";

/// A held advisory lock on one scratch directory, released when dropped
/// (or when the process dies, however it dies — that is the point).
///
/// **Hold this for as long as the scratch directory is in use.**
/// Dropping it early releases the lock immediately, and the *next*
/// Aurora process to start will then correctly conclude the directory is
/// dead and delete it — while it is still being written to.
#[derive(Debug)]
pub struct ScratchLock {
    /// Kept solely to own the descriptor the lock is attached to. The
    /// lock's lifetime *is* this `File`'s lifetime.
    #[allow(dead_code)]
    file: std::fs::File,
}

/// Takes the exclusive, non-blocking `flock` on an already-existing
/// `path`, never creating it — the read-only probe
/// [`sweep_orphaned_scratch_dirs`] reasons about liveness with, and the
/// second half of [`lock_scratch_dir`].
///
/// **No `O_CREAT`, deliberately.** A probe that could create the file it
/// is asking about would turn "this directory has no lock file" into
/// "this directory has a lock file I just made and can obviously lock",
/// i.e. a `Dead` verdict for a directory nothing ever proved dead.
#[cfg(unix)]
fn try_lock_existing(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    flock_exclusive(&file)?;
    Ok(file)
}

/// `flock(LOCK_EX | LOCK_NB)` on `file`, as a `Result`.
#[cfg(unix)]
fn flock_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd as _;

    // SAFETY: `file`'s descriptor is open and owned by the caller's
    // `File` for the whole call, and every caller keeps that `File`
    // alive for exactly as long as the lock is meant to be held -- so
    // the lock cannot outlive the descriptor it was taken on, and no
    // other code in this crate can close it out from under the lock.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Creates a uniquely named, exclusively created, already-`flock`ed file
/// inside `dir`, and returns it with its path — the unpublished half of
/// [`lock_scratch_dir`]'s create-lock-publish sequence.
///
/// The name is not [`LOCK_FILE_NAME`] and is not a `.tile` file, so it is
/// invisible both to the sweep (which only looks for the canonical name)
/// and to every tile enumerator in this workspace. It exists for at most
/// the few microseconds between the `flock` and the `link` that publishes
/// it; a hard crash inside that window leaves one behind, which the
/// scratch directory's own wholesale removal collects like anything else
/// in there.
#[cfg(unix)]
fn create_locked_temp(dir: &Path) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Distinguishes two concurrent attempts from *this* process, which
    /// a pid and a clock reading on their own would not always.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let mut last = std::io::Error::other("no attempt was made");
    for _ in 0..8u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let path = dir.join(format!(
            ".{LOCK_FILE_NAME}.tmp.{}.{}.{nanos}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => {
                // A file this call has just exclusively created cannot
                // already be locked by anyone, so this does not block
                // and does not realistically fail -- but a failure is
                // still reported rather than assumed away.
                if let Err(err) = flock_exclusive(&file) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(err);
                }
                return Ok((file, path));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                last = err;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last)
}

/// Takes the exclusive, non-blocking lock for the scratch directory
/// `dir`, creating its lock file if needed.
///
/// Call this immediately after creating `dir`, and keep the returned
/// guard alive for as long as the directory is in use — see
/// [`ScratchLock`].
///
/// **The lock file is published already locked.** The sequence is
/// *create a uniquely named temporary file → `flock` it → `link(2)` it
/// onto [`LOCK_FILE_NAME`] → unlink the temporary name*. `link` is
/// atomic and fails with `EEXIST` rather than replacing anything, so
/// `aurora.lock` — the one name [`sweep_orphaned_scratch_dirs`] looks
/// for — comes into existence already carrying this call's lock and can
/// never be observed unlocked by a concurrent sweep. The obvious
/// spelling, `open(O_CREAT)` followed by `flock`, is **not** equivalent
/// and was the 0.67.0 bug this replaces: it published the canonical name
/// one syscall before locking it, and a sweep landing in that window
/// acquired the lock itself, concluded `Dead`, and deleted a live
/// session's unsaved pixels.
///
/// `EEXIST` from the `link` means the canonical name is already there —
/// a live session's held lock, or a dead one's leftover — so this falls
/// through to locking (never creating) the file that is actually there,
/// which is what keeps `flock`'s mutual exclusion (and therefore the
/// sweep's `Alive` verdict) meaning what it always did.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the lock file cannot be
/// created or opened (`dir` missing, a symlink in the way — `O_NOFOLLOW`
/// refuses one outright — permissions) or if another open file
/// description already holds the lock (`EWOULDBLOCK`). It never blocks.
#[cfg(unix)]
pub fn lock_scratch_dir(dir: &Path) -> std::io::Result<ScratchLock> {
    let canonical = dir.join(LOCK_FILE_NAME);
    let (file, temp) = create_locked_temp(dir)?;
    match std::fs::hard_link(&temp, &canonical) {
        Ok(()) => {
            // The lock lives on the inode, which now has two names; the
            // canonical one is the only one anything else looks at, so
            // the temporary one is dropped immediately. A failure here
            // leaks one small file inside a directory that is deleted
            // wholesale, so it is logged rather than propagated.
            if let Err(err) = std::fs::remove_file(&temp) {
                tracing::debug!(
                    path = %temp.display(),
                    %err,
                    "could not unlink the temporary lock name after publishing it"
                );
            }
            Ok(ScratchLock { file })
        }
        Err(err) => {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            if err.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(err);
            }
            Ok(ScratchLock {
                file: try_lock_existing(&canonical)?,
            })
        }
    }
}

/// Non-Unix counterpart: a no-op guard.
///
/// Windows has real equivalents (`LockFileEx`, or simply opening the
/// file without `FILE_SHARE_DELETE`), but wiring one up is its own piece
/// of work with its own testing, and a guessed-at implementation here
/// would be worse than an honest gap — see [`sweep_orphaned_scratch_dirs`],
/// which sweeps nothing on this platform, so nothing depends on this
/// guard meaning anything yet.
///
/// # Errors
///
/// Never returns `Err` on this platform. The signature matches the Unix
/// one so callers need no `cfg` of their own.
#[cfg(not(unix))]
pub fn lock_scratch_dir(_dir: &Path) -> std::io::Result<ScratchLock> {
    Ok(ScratchLock {
        file: std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(_dir.join(LOCK_FILE_NAME))?,
    })
}

/// One matching entry's liveness, as an explicit three-way answer.
///
/// The point of naming `Unknown` separately from `Alive` is that they
/// are treated identically (neither is deleted) while meaning very
/// different things — and that no `bool` can be mistakenly coerced into
/// "not alive, therefore dead."
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The lock was acquired: no process holds this directory.
    Dead,
    /// The lock is held by someone: a live session owns this directory.
    Alive,
    /// Anything else at all. Treated exactly like `Alive`.
    Unknown,
}

/// Decides one candidate directory's fate, holding the lock on return
/// for [`Verdict::Dead`] so the caller can delete under it.
#[cfg(unix)]
fn liveness(dir: &Path) -> (Verdict, Option<ScratchLock>) {
    use std::os::unix::fs::MetadataExt as _;

    let Ok(metadata) = std::fs::symlink_metadata(dir) else {
        return (Verdict::Unknown, None);
    };
    // `symlink_metadata`, so a symlink reports as a symlink rather than
    // as whatever it points at -- deleting *through* one is exactly the
    // attack this must not fall for.
    if !metadata.file_type().is_dir() {
        return (Verdict::Unknown, None);
    }
    // SAFETY: `geteuid` takes no arguments, has no preconditions, and
    // cannot fail -- it is inherently safe to call, `std` just does not
    // expose it.
    let euid = unsafe { libc::geteuid() };
    if metadata.uid() != euid {
        return (Verdict::Unknown, None);
    }

    // A missing lock file is `Unknown`, never `Dead`: it is also what a
    // directory created microseconds ago looks like, and what every
    // pre-0.67.0 leftover looks like forever. See this module's own
    // "residual races" list.
    if !dir.join(LOCK_FILE_NAME).is_file() {
        return (Verdict::Unknown, None);
    }

    // `try_lock_existing`, not `lock_scratch_dir`: this must never
    // *create* the file it is reasoning about. If the lock file were to
    // vanish between the check just above and this call (another sweep
    // removing the same directory), a creating variant would make a
    // fresh lock file, lock it trivially, and call a directory nothing
    // ever proved dead `Dead`.
    match try_lock_existing(&dir.join(LOCK_FILE_NAME)) {
        Ok(file) => (Verdict::Dead, Some(ScratchLock { file })),
        Err(err) if err.raw_os_error() == Some(libc::EWOULDBLOCK) => (Verdict::Alive, None),
        Err(_) => (Verdict::Unknown, None),
    }
}

/// Removes scratch directories under `parent` whose names start with
/// `prefix` and whose owning process is provably gone.
///
/// Call this **once, at startup, before this session creates its own
/// scratch directory** — which is what makes "never delete the current
/// session's own directory" true by construction rather than by a check
/// that could be got wrong.
///
/// `parent` is a parameter with no default on purpose: nothing in here
/// reaches for [`std::env::temp_dir`] itself, so a test can point it at
/// a `tempfile::tempdir()` with no possibility of touching a real
/// running Aurora's scratch directory or another test's.
///
/// Nothing is deleted unless its lock is acquired, proving no live
/// process holds it; the lock is held across the removal and dropped
/// after. Every other outcome increments `skipped`. This function never
/// returns an error — a sweep that cannot read `parent` at all reports
/// zero of both and logs, because failing startup over housekeeping
/// would be the wrong trade.
#[cfg(unix)]
#[must_use]
pub fn sweep_orphaned_scratch_dirs(parent: &Path, prefix: &str) -> SweepReport {
    let mut report = SweepReport::default();
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::debug!(
                parent = %parent.display(),
                %err,
                "could not read the scratch parent directory; sweeping nothing"
            );
            return report;
        }
    };

    for entry in entries {
        let Ok(entry) = entry else {
            // An unreadable entry is not even a candidate -- there is no
            // name to match against, so it is not counted either.
            continue;
        };
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(prefix) {
            continue;
        }
        let path = entry.path();
        let (verdict, lock) = liveness(&path);
        if verdict != Verdict::Dead {
            tracing::debug!(
                path = %path.display(),
                ?verdict,
                "leaving a scratch directory alone"
            );
            report.skipped += 1;
            continue;
        }
        // The lock is still held here, deliberately: it is dropped only
        // after the directory is gone, so a process starting mid-removal
        // cannot also conclude "dead" and race us into the same tree.
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                report.removed += 1;
                tracing::info!(
                    path = %path.display(),
                    "removed an orphaned scratch directory from a session that is no longer running"
                );
            }
            Err(err) => {
                report.skipped += 1;
                tracing::warn!(
                    path = %path.display(),
                    %err,
                    "could not remove an orphaned scratch directory"
                );
            }
        }
        drop(lock);
    }
    report
}

/// Non-Unix counterpart: sweeps nothing and says so, once.
///
/// The liveness check this rests on is `flock`, which is Unix-only; the
/// Windows equivalent is real, separate work (see [`lock_scratch_dir`]'s
/// own non-Unix arm). Reporting `SweepReport::default()` rather than
/// guessing keeps the caller's logged counts truthful on every platform.
#[cfg(not(unix))]
#[must_use]
pub fn sweep_orphaned_scratch_dirs(parent: &Path, prefix: &str) -> SweepReport {
    tracing::info!(
        parent = %parent.display(),
        prefix,
        "scratch-directory liveness is not implemented on this platform; sweeping nothing"
    );
    SweepReport::default()
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        LOCK_FILE_NAME, ScratchLock, lock_scratch_dir, sweep_orphaned_scratch_dirs,
        try_lock_existing,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const PREFIX: &str = "aurora-scratch-";

    /// A scratch-shaped directory under `parent`, with one file in it so
    /// a wrong "removed" verdict is visibly destructive rather than a
    /// no-op on an empty directory.
    fn scratch_dir(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        if let Err(err) = std::fs::create_dir(&dir) {
            unreachable!("a test-local temp directory must accept a mkdir: {err}");
        }
        if let Err(err) = std::fs::write(dir.join("0_0_0.tile"), b"pretend pixels") {
            unreachable!("a test-local temp directory must accept a write: {err}");
        }
        dir
    }

    fn take_lock(dir: &Path) -> ScratchLock {
        match lock_scratch_dir(dir) {
            Ok(lock) => lock,
            Err(err) => unreachable!("a freshly created directory must be lockable: {err}"),
        }
    }

    fn temp_parent() -> tempfile::TempDir {
        match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("a temp directory must be creatable: {err}"),
        }
    }

    /// The whole point of the feature: a directory whose owning process
    /// is gone. "Gone" is modelled by taking the lock and dropping it,
    /// which is exactly what the kernel does when a process dies.
    #[test]
    fn a_directory_whose_owner_has_exited_is_removed() {
        let parent = temp_parent();
        let dir = scratch_dir(parent.path(), "aurora-scratch-dead");
        drop(take_lock(&dir));

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 1);
        assert_eq!(report.skipped, 0);
        assert!(!dir.exists(), "the orphan must actually be gone");
    }

    /// The case that must never regress: a *live* session's directory.
    ///
    /// No child process is needed — `flock` attaches to the open file
    /// description, so a lock this test still holds conflicts with the
    /// sweep's own attempt exactly as another process's would.
    #[test]
    fn a_directory_whose_lock_is_still_held_is_left_completely_alone() {
        let parent = temp_parent();
        let dir = scratch_dir(parent.path(), "aurora-scratch-live");
        let live = take_lock(&dir);

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 0, "a live session's pixels are not garbage");
        assert_eq!(report.skipped, 1);
        assert!(dir.exists());
        assert!(dir.join("0_0_0.tile").is_file(), "and its tiles survive");
        drop(live);
    }

    /// A pre-0.67.0 leftover, and also what a directory looks like in
    /// the microseconds between `mkdir` and `flock`. Both are `Unknown`,
    /// and `Unknown` never deletes.
    #[test]
    fn a_directory_with_no_lock_file_is_skipped_not_swept() {
        let parent = temp_parent();
        let dir = scratch_dir(parent.path(), "aurora-scratch-legacy");

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 0);
        assert_eq!(report.skipped, 1);
        assert!(dir.exists());
    }

    /// A lock *path* that cannot be opened as a file at all.
    ///
    /// A directory in its place fails deterministically with `EISDIR`
    /// for every user including root, unlike a permission-based fixture,
    /// which behaves differently when the suite happens to run as root.
    #[test]
    fn a_directory_whose_lock_path_cannot_be_opened_is_skipped_not_swept() {
        let parent = temp_parent();
        let dir = scratch_dir(parent.path(), "aurora-scratch-eisdir");
        if let Err(err) = std::fs::create_dir(dir.join(LOCK_FILE_NAME)) {
            unreachable!("a test-local temp directory must accept a mkdir: {err}");
        }

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 0, "an error must never become a deletion");
        assert_eq!(report.skipped, 1);
        assert!(dir.exists());
    }

    /// The sweep is scoped by prefix, so a temp directory belonging to
    /// anything else is not even a candidate — not counted, not touched.
    #[test]
    fn a_directory_whose_name_does_not_match_the_prefix_is_untouched() {
        let parent = temp_parent();
        let other = scratch_dir(parent.path(), "someone-elses-tmpdir");
        drop(take_lock(&other));

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 0);
        assert_eq!(report.skipped, 0, "a non-match is not even counted");
        assert!(other.exists());
    }

    /// A plain *file* carrying the prefix is not a scratch directory, so
    /// it is skipped rather than unlinked — `remove_dir_all` on it would
    /// be a deletion this function has no business performing.
    #[test]
    fn a_plain_file_with_the_matching_prefix_is_untouched() {
        let parent = temp_parent();
        let path = parent.path().join("aurora-scratch-notadir");
        if let Err(err) = std::fs::write(&path, b"not a directory") {
            unreachable!("a test-local temp directory must accept a write: {err}");
        }

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 0);
        assert_eq!(report.skipped, 1);
        assert!(path.is_file());
    }

    /// The lock really is exclusive within one process, which is what
    /// makes the "live" test above a valid stand-in for a second
    /// process, and what makes `flock` (per open file description) the
    /// right primitive rather than `fcntl` (per process).
    #[test]
    fn a_second_lock_attempt_on_the_same_directory_is_refused() {
        let parent = temp_parent();
        let dir = scratch_dir(parent.path(), "aurora-scratch-twice");
        let first = take_lock(&dir);

        match lock_scratch_dir(&dir) {
            Ok(_) => unreachable!("a held lock must not be handed out twice"),
            Err(err) => assert_eq!(err.raw_os_error(), Some(libc::EWOULDBLOCK)),
        }

        drop(first);
        // And it becomes available again once released, so a crashed
        // session's directory really does become sweepable.
        drop(take_lock(&dir));
    }

    /// **The 0.67.0 data-loss race, as a test.**
    ///
    /// `lock_scratch_dir` used to `open(O_CREAT)` the canonical lock file
    /// and *then* `flock` it, so between those two syscalls `aurora.lock`
    /// existed unlocked — and a sweep landing in that window acquired the
    /// lock itself, concluded [`super::Verdict::Dead`], and deleted the
    /// directory of a session that was at that instant still taking its
    /// own lock.
    ///
    /// This asserts the invariant that makes that unreachable rather than
    /// merely unlikely: **while a `ScratchLock` is held, no observer can
    /// ever acquire the lock on the canonical file** — including during
    /// the acquisition itself. Two poller threads spin on
    /// `try_lock_existing` (which never creates, exactly as the sweep's
    /// own probe does not) across the whole acquisition, and the guard is
    /// released only after they have stopped, so *any* successful probe
    /// is a genuine violation and there are no false positives.
    ///
    /// It is a race detector, so a green run is evidence and not proof —
    /// which is why it also asserts the pollers actually saw the file at
    /// all, so a run that raced nothing cannot pass silently. Measured
    /// against the pre-fix code (the `open`-then-`flock` spelling
    /// restored temporarily): it failed, reporting violations.
    #[test]
    fn the_canonical_lock_file_is_never_visible_before_it_is_locked() {
        const ITERATIONS: usize = 200;
        const POLLERS: usize = 2;

        let parent = temp_parent();
        let violations = Arc::new(AtomicUsize::new(0));
        let sightings = Arc::new(AtomicUsize::new(0));

        for i in 0..ITERATIONS {
            let dir = scratch_dir(parent.path(), &format!("aurora-scratch-race-{i}"));
            let stop = Arc::new(AtomicBool::new(false));
            let pollers: Vec<_> = (0..POLLERS)
                .map(|_| {
                    let lock_path = dir.join(LOCK_FILE_NAME);
                    let stop = Arc::clone(&stop);
                    let violations = Arc::clone(&violations);
                    let sightings = Arc::clone(&sightings);
                    std::thread::spawn(move || {
                        while !stop.load(Ordering::Relaxed) {
                            match try_lock_existing(&lock_path) {
                                Ok(file) => {
                                    // The acquirer below has not released
                                    // anything yet, so this can only mean
                                    // the canonical name became visible
                                    // before it was locked.
                                    violations.fetch_add(1, Ordering::Relaxed);
                                    sightings.fetch_add(1, Ordering::Relaxed);
                                    drop(file);
                                }
                                Err(err) if err.raw_os_error() == Some(libc::EWOULDBLOCK) => {
                                    sightings.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {}
                            }
                        }
                    })
                })
                .collect();

            let held = take_lock(&dir);
            stop.store(true, Ordering::Relaxed);
            for poller in pollers {
                if poller.join().is_err() {
                    unreachable!("a poller thread must not panic");
                }
            }
            // Only now, with every observer stopped.
            drop(held);
        }

        assert_eq!(
            violations.load(Ordering::Relaxed),
            0,
            "the canonical lock file was observed unlocked while a ScratchLock was held"
        );
        assert!(
            sightings.load(Ordering::Relaxed) > 0,
            "the pollers never saw the lock file at all, so this run raced nothing"
        );
    }

    /// The publish step's own guarantee, stated directly: once
    /// `lock_scratch_dir` returns, the canonical name exists and is the
    /// very inode the returned guard holds — not a second, unlocked file
    /// left beside it — and the temporary name it was published from is
    /// gone.
    #[test]
    fn the_lock_file_that_ends_up_published_is_the_one_that_is_locked() {
        let parent = temp_parent();
        let dir = scratch_dir(parent.path(), "aurora-scratch-published");
        let held = take_lock(&dir);

        let canonical = dir.join(LOCK_FILE_NAME);
        assert!(canonical.is_file(), "the canonical name must exist");
        match try_lock_existing(&canonical) {
            Ok(_) => unreachable!("the published lock file must already be locked"),
            Err(err) => assert_eq!(err.raw_os_error(), Some(libc::EWOULDBLOCK)),
        }

        let Ok(entries) = std::fs::read_dir(&dir) else {
            unreachable!("a test-local temp directory must be readable");
        };
        let leftovers: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&format!(".{LOCK_FILE_NAME}.tmp.")))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the temporary lock name must not survive publication: {leftovers:?}"
        );
        drop(held);
    }

    /// The mixed case, in one sweep: the report's two counters must
    /// track the right entries, not merely add up.
    #[test]
    fn a_sweep_removes_only_the_dead_among_several_candidates() {
        let parent = temp_parent();
        let dead_one = scratch_dir(parent.path(), "aurora-scratch-a");
        let dead_two = scratch_dir(parent.path(), "aurora-scratch-b");
        let alive = scratch_dir(parent.path(), "aurora-scratch-c");
        let legacy = scratch_dir(parent.path(), "aurora-scratch-d");
        drop(take_lock(&dead_one));
        drop(take_lock(&dead_two));
        let held = take_lock(&alive);

        let report = sweep_orphaned_scratch_dirs(parent.path(), PREFIX);

        assert_eq!(report.removed, 2);
        assert_eq!(report.skipped, 2);
        assert!(!dead_one.exists());
        assert!(!dead_two.exists());
        assert!(alive.exists());
        assert!(legacy.exists());
        drop(held);
    }
}
