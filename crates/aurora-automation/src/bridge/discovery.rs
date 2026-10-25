//! The discovery file: how the proxy finds the running app.
//!
//! The app writes `aurora-bridge-<pid>.json` (port, token, pid, version)
//! into a **private per-user directory** (`0700`), with mode `0600`, created
//! with `create_new` (an existing file is never replaced), next to a lock
//! file `aurora-bridge-<pid>.lock` it holds for as long as it runs. Both are
//! removed on a clean exit. After a crash they remain; the next app (or
//! proxy) sweeps them with the scratch directory's rule: **only** a regular
//! file with exactly that name shape, that parses as a discovery record
//! naming the same pid, whose lock file exists and can be taken (so its
//! owner is gone). A lock that is held (a live app), a missing lock file, a
//! symbolic link, a file that is not ours and every other name are left
//! alone.
//!
//! Default directory: `$XDG_RUNTIME_DIR/aurora` (Linux), else
//! `<temp>/aurora-bridge-<uid>` (Linux fallback, macOS: `$TMPDIR` is already
//! per-user), `%LOCALAPPDATA%\Aurora\bridge` (Windows). A directory that is
//! a symbolic link, not owned by the current user, or readable by anyone
//! else is refused (Unix); on Windows the user-profile directory's own ACL
//! is what protects it (not verified here).
//!
//! **What this does not protect:** any process running as the same user can
//! read the file. That is by design: the token is a capability handed to
//! "whoever can run as this user", the same authority they already have over
//! the app's files and memory.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::protocol::PROTOCOL_VERSION;
use super::token;

/// The `magic` field of a discovery file.
pub const MAGIC: &str = "aurora-bridge-discovery";
/// File name prefix.
pub const FILE_PREFIX: &str = "aurora-bridge-";
/// Environment variable overriding the default directory (tests, and a user
/// who wants the files elsewhere; the directory is still verified).
pub const DIR_ENV: &str = "AURORA_BRIDGE_DIR";
/// Discovery files are tiny; anything larger is not one.
pub const MAX_DISCOVERY_BYTES: u64 = 4096;

/// The record in a discovery file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub magic: String,
    pub protocol: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub version: String,
    pub started_unix: u64,
}

impl Discovery {
    /// A record for this process.
    #[must_use]
    pub fn new(port: u16, token: String, version: &str) -> Self {
        Self {
            magic: MAGIC.to_owned(),
            protocol: PROTOCOL_VERSION,
            port,
            token,
            pid: std::process::id(),
            version: version.to_owned(),
            started_unix: std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        }
    }

    /// Whether the record is one this crate would have written.
    ///
    /// # Errors
    /// What is wrong with it.
    pub fn validate(&self) -> Result<(), String> {
        if self.magic != MAGIC {
            return Err("not an Aurora bridge discovery file".to_owned());
        }
        if self.protocol != PROTOCOL_VERSION {
            return Err(format!(
                "the app speaks bridge protocol {}, this tool speaks {PROTOCOL_VERSION}",
                self.protocol
            ));
        }
        if self.port == 0 {
            return Err("the discovery file has no port".to_owned());
        }
        if !token::well_formed(&self.token) {
            return Err("the discovery file's token is malformed".to_owned());
        }
        Ok(())
    }
}

/// Why a discovery file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error(
        "no running Aurora with the bridge enabled was found in {0} (start it with --mcp-bridge)"
    )]
    NotFound(String),
    #[error("{0}")]
    Unusable(String),
    #[error("discovery file i/o: {0}")]
    Io(String),
}

// ---------------------------------------------------------------------------
// Directory
// ---------------------------------------------------------------------------

/// The default directory for discovery files on this platform, if one can be
/// worked out.
#[must_use]
pub fn default_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        return std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|base| PathBuf::from(base).join("Aurora").join("bridge"));
    }
    #[cfg(not(windows))]
    {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir())
        {
            return Some(runtime.join("aurora"));
        }
        #[cfg(unix)]
        {
            let uid = rustix::process::getuid().as_raw();
            Some(std::env::temp_dir().join(format!("aurora-bridge-{uid}")))
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

#[cfg(unix)]
fn my_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Creates `dir` (and missing parents) if needed and checks it is private:
/// a real directory (never a symbolic link), owned by the current user,
/// with no group or other permission bits. A directory the user owns with
/// looser bits is tightened to `0700`.
///
/// # Errors
/// The directory cannot be made private, or belongs to someone else.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a plain directory (a symbolic link?)",
                dir.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        if meta.uid() != my_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} belongs to another user", dir.display()),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            let again = std::fs::symlink_metadata(dir)?;
            if again.mode() & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is accessible to other users", dir.display()),
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

fn json_name(pid: u32) -> String {
    format!("{FILE_PREFIX}{pid}.json")
}

fn lock_name(pid: u32) -> String {
    format!("{FILE_PREFIX}{pid}.lock")
}

/// The pid in a name of the exact shape `aurora-bridge-<digits><suffix>`.
fn pid_of(name: &str, suffix: &str) -> Option<u32> {
    let digits = name.strip_prefix(FILE_PREFIX)?.strip_suffix(suffix)?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    digits.parse().ok()
}

// ---------------------------------------------------------------------------
// Publishing (the app)
// ---------------------------------------------------------------------------

/// A published discovery file. Dropping it removes the file and releases
/// the lock (a clean exit); a crash leaves both for the next sweep.
#[derive(Debug)]
pub struct Published {
    json: PathBuf,
    lock_path: PathBuf,
    lock: Option<File>,
}

impl Published {
    /// Where the discovery file is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.json
    }

    /// Removes the files now (disabling the bridge).
    pub fn remove(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.json)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, "couldn't remove the bridge discovery file");
        }
        // Release the lock before removing its file (Windows).
        self.lock = None;
        if let Err(e) = std::fs::remove_file(&self.lock_path)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, "couldn't remove the bridge lock file");
        }
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        self.remove();
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed());
    }
    options
}

fn make_owner_only(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

/// Publishes `record` in `dir`: makes the directory private, sweeps dead
/// apps' files, takes this process's lock file and creates the discovery
/// file (never replacing one).
///
/// # Errors
/// The directory is not private, a file for this pid already exists and is
/// live, or any write failed.
pub fn publish(dir: &Path, record: &Discovery) -> io::Result<Published> {
    record
        .validate()
        .map_err(|m| io::Error::new(io::ErrorKind::InvalidInput, m))?;
    ensure_private_dir(dir)?;
    let report = sweep_stale(dir);
    if report.removed > 0 {
        tracing::info!(
            removed = report.removed,
            "removed stale bridge discovery files"
        );
    }
    let lock_path = dir.join(lock_name(record.pid));
    let json = dir.join(json_name(record.pid));
    let mut lock_options = private_options();
    lock_options.create(true).truncate(false);
    let lock = lock_options.open(&lock_path)?;
    make_owner_only(&lock)?;
    lock.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::AlreadyExists,
            "another bridge with this process id is running",
        ),
        std::fs::TryLockError::Error(e) => e,
    })?;
    let mut file_options = private_options();
    file_options.create_new(true);
    let mut file = file_options.open(&json)?;
    // Handed to the guard first, so a failure below still removes the file.
    let mut published = Published {
        json,
        lock_path,
        lock: Some(lock),
    };
    make_owner_only(&file)?;
    let text = serde_json::to_string(record).map_err(|e| io::Error::other(e.to_string()))?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    let _ = &mut published;
    Ok(published)
}

// ---------------------------------------------------------------------------
// Reading and sweeping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockState {
    /// A live process holds it.
    Held,
    /// The file exists and nobody holds it: its owner is gone.
    Free,
    /// No (regular) lock file.
    Missing,
}

fn lock_state(lock_path: &Path) -> LockState {
    let Ok(meta) = std::fs::symlink_metadata(lock_path) else {
        return LockState::Missing;
    };
    if !meta.file_type().is_file() {
        return LockState::Missing;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed());
    }
    let Ok(file) = options.open(lock_path) else {
        return LockState::Missing;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            LockState::Free
        }
        Err(std::fs::TryLockError::WouldBlock) => LockState::Held,
        Err(std::fs::TryLockError::Error(_)) => LockState::Missing,
    }
}

/// Reads a discovery file, refusing one that is not a plain file of ours
/// with owner-only permissions (Unix), that is oversized, or that does not
/// validate.
///
/// # Errors
/// [`DiscoveryError::Unusable`] with the reason.
pub fn read_file(path: &Path) -> Result<Discovery, DiscoveryError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| DiscoveryError::Io(e.to_string()))?;
    if !meta.file_type().is_file() {
        return Err(DiscoveryError::Unusable(format!(
            "{} is not a plain file",
            path.display()
        )));
    }
    if meta.len() > MAX_DISCOVERY_BYTES {
        return Err(DiscoveryError::Unusable(format!(
            "{} is too large to be a discovery file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.uid() != my_uid() {
            return Err(DiscoveryError::Unusable(format!(
                "{} belongs to another user; refusing to read a token from it",
                path.display()
            )));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(DiscoveryError::Unusable(format!(
                "{} is readable by other users (mode {:o}); refusing to use it",
                path.display(),
                meta.mode() & 0o777
            )));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed());
    }
    let file = options
        .open(path)
        .map_err(|e| DiscoveryError::Io(e.to_string()))?;
    let mut text = String::new();
    file.take(MAX_DISCOVERY_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| DiscoveryError::Io(e.to_string()))?;
    let record: Discovery = serde_json::from_str(&text)
        .map_err(|e| DiscoveryError::Unusable(format!("{}: {e}", path.display())))?;
    record
        .validate()
        .map_err(|m| DiscoveryError::Unusable(format!("{}: {m}", path.display())))?;
    Ok(record)
}

/// What a sweep did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub removed: usize,
    pub skipped: usize,
}

/// Removes the discovery files of apps that are gone, and nothing else (see
/// the module docs for every condition).
pub fn sweep_stale(dir: &Path) -> SweepReport {
    let mut report = SweepReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(pid) = pid_of(name, ".json") {
            let path = entry.path();
            if sweep_one(dir, &path, pid) {
                report.removed += 1;
            } else {
                report.skipped += 1;
            }
        } else if let Some(pid) = pid_of(name, ".lock") {
            // An orphan lock file: its json is gone (a crash between the
            // steps, or a half-finished sweep).
            let json = dir.join(json_name(pid));
            if std::fs::symlink_metadata(&json).is_err() && orphan_lock_removable(&entry.path()) {
                if std::fs::remove_file(entry.path()).is_ok() {
                    report.removed += 1;
                }
            }
        }
    }
    report
}

fn orphan_lock_removable(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.uid() != my_uid() {
            return false;
        }
    }
    meta.file_type().is_file() && meta.len() == 0 && lock_state(path) == LockState::Free
}

fn sweep_one(dir: &Path, path: &Path, pid: u32) -> bool {
    // Ours, a regular file, small, parses, names the same pid.
    let Ok(record) = read_file(path) else {
        return false;
    };
    if record.pid != pid {
        return false;
    }
    let lock_path = dir.join(lock_name(pid));
    if lock_state(&lock_path) != LockState::Free {
        return false;
    }
    let removed = std::fs::remove_file(path).is_ok();
    if removed {
        let _ = std::fs::remove_file(&lock_path);
    }
    removed
}

/// The running app's discovery record: the newest valid file in `dir`
/// whose lock is held (its app is alive).
///
/// # Errors
/// Nothing found, or the only candidates are unusable (the reason is
/// reported).
pub fn find_running(dir: &Path) -> Result<(PathBuf, Discovery), DiscoveryError> {
    let entries =
        std::fs::read_dir(dir).map_err(|_| DiscoveryError::NotFound(dir.display().to_string()))?;
    let mut best: Option<(std::time::SystemTime, PathBuf, Discovery)> = None;
    let mut last_problem: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| pid_of(n, ".json")) else {
            continue;
        };
        let path = entry.path();
        match read_file(&path) {
            Ok(record) if record.pid == pid => {
                if lock_state(&dir.join(lock_name(pid))) != LockState::Held {
                    continue;
                }
                let modified = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _, _)| modified > *t) {
                    best = Some((modified, path, record));
                }
            }
            Ok(_) => last_problem = Some(format!("{} names a different process", path.display())),
            Err(e) => last_problem = Some(e.to_string()),
        }
    }
    match best {
        Some((_, path, record)) => Ok((path, record)),
        None => Err(last_problem.map_or_else(
            || DiscoveryError::NotFound(dir.display().to_string()),
            DiscoveryError::Unusable,
        )),
    }
}
