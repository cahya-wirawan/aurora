//! Per-session crash-recovery autosave files and their index (0.171.0).
//!
//! Until 0.171.0 a run wrote one fixed file, `aurora-autosave.aur`, in
//! `std::env::temp_dir()`. A later round opens several documents at once,
//! and each must be recoverable after a crash, so every document session
//! now has its own file and a small index names them. Everything lives in
//! the same directory as before, under a per-run **key** ([`RunKey`]):
//! the run's pid, plus a generation that is only non-zero when a crashed
//! run with the same pid still has files, so a reused pid never shares a
//! crashed run's namespace (0.171.0 review M1).
//!
//! - `aurora-autosave-<key>-<docid>.aur` — one session's `.aur` container
//!   (its `.partial.aur` sibling keeps the 0.52.2 meaning), written by
//!   `crate::background_autosave` exactly as the single file was.
//! - `aurora-autosave-<key>.index` — the run's index: magic, version, the
//!   active id and every session in tab order with its file name
//!   ([`encode_index`]). Written to a unique temp file, synced and renamed
//!   ([`write_index`]), and only ever naming files that have already
//!   landed complete ([`IndexBook`]), so a crash between a session's file
//!   and the index leaves the previous index — and the previous good state
//!   — in place.
//! - `aurora-autosave-<key>.lock` — held with an exclusive OS file lock
//!   (`std::fs::File::try_lock`) for the run's whole life. A file lock is
//!   released by the OS when its process dies, however it dies, so a lock
//!   another run can take is the proof that its owner is gone
//!   ([`claim_sources`]). That is what lets a run tell a crashed run's
//!   files from those of a second Aurora that is still running, and why a
//!   run only ever deletes its own namespace or one whose lock it holds.
//! - `aurora-session.marker` — unchanged location and meaning ("a run that
//!   has not shut down cleanly"), but it now records keys
//!   ([`encode_marker`]): the writer's own first, then the other runs that
//!   still have files. The recovering run has a different pid from the
//!   crashed one, so this is how it finds the crashed run's index, and why
//!   a crash *during* recovery still finds the older files. A clean quit
//!   rewrites it without its own key and deletes it only when no listed
//!   run has anything left ([`marker_after_quit`]). An empty marker
//!   (pre-0.171.0) means "no key", and recovery falls back to the legacy
//!   single file.
//!
//! Recovery ([`plan_recovery`]) reads each crashed run's index; a missing
//! or damaged index falls back to scanning that run's namespace
//! ([`scan_namespace`]); and when no source names anything, the legacy
//! `aurora-autosave.aur` from a pre-0.171.0 crash is used.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// How often a live run touches its lock file and index (0.171.1, L-a):
/// on every autosave landing, and on this timer on the autosave thread.
/// `systemd-tmpfiles` removes `/tmp` files untouched for 10 days by
/// default, so a refreshed lock is never old enough to be cleaned.
pub(crate) const LIVENESS_REFRESH: Duration = Duration::from_mins(30);

/// A run whose lock file is missing but whose index or session files were
/// modified within this window is treated as possibly alive and left
/// alone (0.171.1, L-a). Four refresh intervals: a live run's index is
/// at most one interval old (plus scheduling delay), and a crashed run's
/// files stop being touched, so they age out of it and the run is
/// recovered after at most two hours — never blocked forever.
pub(crate) const LIVENESS_WINDOW: Duration = Duration::from_hours(2);

/// Every autosave file name starts with this.
const PREFIX: &str = "aurora-autosave";

/// The index's first line, before its version.
pub(crate) const INDEX_MAGIC: &str = "aurora-autosave-index";

/// The index format this build writes and reads.
pub(crate) const INDEX_VERSION: u32 = 1;

/// The marker's first line, before its version.
const MARKER_MAGIC: &str = "aurora-session";

/// An index longer than this is damaged, not read: a run lists its open
/// documents, never thousands.
const INDEX_MAX_BYTES: u64 = 1 << 20;

/// More sessions than this in one index is damaged.
const INDEX_MAX_ENTRIES: usize = 4096;

/// Only this much of a marker is read (0.171.0 review L2): the temp
/// directory is shared, and the marker is anyone's to write.
const MARKER_MAX_BYTES: u64 = 64 << 10;

/// At most this many keys are parsed out of a marker's bytes.
const MARKER_MAX_PARSED: usize = 16 << 10;

/// At most this many keys are recovered from, inherited or written back
/// (0.171.0 review N2: was 64). Keys with an index or session files are
/// kept ahead of lock-only ones when there are more ([`prioritised`]), so
/// the cap can never drop a run that still has documents for one that
/// has only a lock. 1024 keys of at most 21 bytes stay inside
/// [`MARKER_MAX_BYTES`].
const MARKER_MAX_KEYS: usize = 1024;

/// How many generations a pid may have before a run gives up looking for
/// a free one (it then runs without a lock, logged).
const MAX_GENERATIONS: u32 = 1024;

/// One run's namespace: its pid and a generation, written `<pid>` for
/// generation 0 and `<pid>.<generation>` otherwise, so the common case
/// is the plain `aurora-autosave-<pid>-<docid>.aur`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RunKey {
    pub(crate) pid: u32,
    pub(crate) generation: u32,
}

impl From<u32> for RunKey {
    fn from(pid: u32) -> Self {
        Self { pid, generation: 0 }
    }
}

impl std::fmt::Display for RunKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.generation == 0 {
            write!(f, "{}", self.pid)
        } else {
            write!(f, "{}.{}", self.pid, self.generation)
        }
    }
}

impl RunKey {
    /// Parses [`RunKey`]'s own spelling, and only that one: `12`, `12.3`;
    /// never `12.0`, `012` or `+12`, so one key has one file name.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (pid, generation) = match text.split_once('.') {
            Some((pid, generation)) => (pid, Some(generation)),
            None => (text, None),
        };
        if !all_digits(pid) || !generation.is_none_or(all_digits) {
            return None;
        }
        let key = Self {
            pid: pid.parse().ok()?,
            generation: generation.map_or(Some(0), |g| g.parse().ok())?,
        };
        (key.to_string() == text).then_some(key)
    }
}

/// The pre-0.171.0 single autosave file.
pub(crate) fn legacy_path(dir: &Path) -> PathBuf {
    dir.join(format!("{PREFIX}.aur"))
}

/// One session's canonical file name.
pub(crate) fn session_file_name(key: impl Into<RunKey>, id: u64) -> String {
    format!("{PREFIX}-{}-{id}.aur", key.into())
}

/// A run's index path.
pub(crate) fn index_path(dir: &Path, key: impl Into<RunKey>) -> PathBuf {
    dir.join(format!("{PREFIX}-{}.index", key.into()))
}

/// A run's liveness-lock path.
pub(crate) fn lock_path(dir: &Path, key: impl Into<RunKey>) -> PathBuf {
    dir.join(format!("{PREFIX}-{}.lock", key.into()))
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `(key, id, partial)` for a session file name — canonical
/// (`…-<key>-<id>.aur`) or partial (`…-<key>-<id>.partial.aur`); `None`
/// for anything else, temp files included.
pub(crate) fn parse_session_file_name(name: &str) -> Option<(RunKey, u64, bool)> {
    let rest = name.strip_prefix(PREFIX)?.strip_prefix('-')?;
    let (key, rest) = rest.split_once('-')?;
    let (id, partial) = match rest.strip_suffix(".partial.aur") {
        Some(id) => (id, true),
        None => (rest.strip_suffix(".aur")?, false),
    };
    if !all_digits(id) {
        return None;
    }
    Some((RunKey::parse(key)?, id.parse().ok()?, partial))
}

/// One session the index names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexEntry {
    pub(crate) id: u64,
    /// The canonical file name, relative to the index's own directory.
    pub(crate) file: String,
}

/// What an index records: the sessions in tab order and the active one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AutosaveIndex {
    pub(crate) active: Option<u64>,
    pub(crate) entries: Vec<IndexEntry>,
}

impl AutosaveIndex {
    /// The entries with the active one first, then the rest in tab order:
    /// the order recovery tries them in.
    pub(crate) fn recovery_order(&self) -> Vec<IndexEntry> {
        let mut ordered: Vec<IndexEntry> = self
            .entries
            .iter()
            .filter(|entry| Some(entry.id) == self.active)
            .cloned()
            .collect();
        ordered.extend(
            self.entries
                .iter()
                .filter(|entry| Some(entry.id) != self.active)
                .cloned(),
        );
        ordered
    }
}

/// The index as text. An `end` line closes it, so a truncated file is
/// detected rather than read as a shorter list.
pub(crate) fn encode_index(index: &AutosaveIndex) -> String {
    let mut lines = vec![
        format!("{INDEX_MAGIC} {INDEX_VERSION}"),
        index
            .active
            .map_or_else(|| "active -".to_owned(), |id| format!("active {id}")),
    ];
    lines.extend(
        index
            .entries
            .iter()
            .map(|entry| format!("session {} {}", entry.id, entry.file)),
    );
    lines.push("end".to_owned());
    lines.push(String::new());
    lines.join("\n")
}

/// Why an index could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexError {
    /// No index file.
    Missing,
    /// The file exists but is not a whole, valid index.
    Damaged(String),
}

fn damaged(reason: impl Into<String>) -> IndexError {
    IndexError::Damaged(reason.into())
}

/// Parses [`encode_index`]'s text for run `key`, strictly: wrong magic or
/// version, a missing `end`, anything after it, a file name that is not
/// a canonical session name of **this run's own key** (so never a path,
/// and never another run's file — 0.171.0 review L1), an id that
/// disagrees with its file name, a repeated id, or an active id that
/// names no entry is damage.
pub(crate) fn decode_index(text: &str, key: RunKey) -> Result<AutosaveIndex, IndexError> {
    let mut lines = text.split('\n');
    let header = lines.next().unwrap_or_default();
    if header != format!("{INDEX_MAGIC} {INDEX_VERSION}") {
        return Err(damaged("bad magic or version"));
    }
    let active = match lines.next().and_then(|line| line.strip_prefix("active ")) {
        Some("-") => None,
        Some(id) if all_digits(id) => Some(
            id.parse::<u64>()
                .map_err(|_| damaged("active id out of range"))?,
        ),
        _ => return Err(damaged("no active line")),
    };
    let mut entries = Vec::new();
    let mut ids = BTreeSet::new();
    let mut ended = false;
    for line in lines.by_ref() {
        if line == "end" {
            ended = true;
            break;
        }
        let Some((id, file)) = line
            .strip_prefix("session ")
            .and_then(|rest| rest.split_once(' '))
        else {
            return Err(damaged("bad session line"));
        };
        let id: u64 = if all_digits(id) {
            id.parse().map_err(|_| damaged("session id out of range"))?
        } else {
            return Err(damaged("bad session id"));
        };
        match parse_session_file_name(file) {
            Some((file_key, file_id, false)) if file_id == id && file_key == key => {}
            _ => return Err(damaged("bad session file name")),
        }
        if !ids.insert(id) || ids.len() > INDEX_MAX_ENTRIES {
            return Err(damaged("repeated id or too many sessions"));
        }
        entries.push(IndexEntry {
            id,
            file: file.to_owned(),
        });
    }
    if !ended {
        return Err(damaged("truncated: no end line"));
    }
    // Only the final newline may follow `end`.
    if lines.any(|line| !line.is_empty()) {
        return Err(damaged("data after the end line"));
    }
    if let Some(active) = active
        && !ids.contains(&active)
    {
        return Err(damaged("active id names no session"));
    }
    Ok(AutosaveIndex { active, entries })
}

/// Reads and decodes run `key`'s index at `path`, bounded by
/// [`INDEX_MAX_BYTES`].
pub(crate) fn read_index(path: &Path, key: RunKey) -> Result<AutosaveIndex, IndexError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(IndexError::Missing),
        Err(err) => return Err(damaged(format!("unreadable: {err}"))),
    };
    let mut bytes = Vec::new();
    if let Err(err) = file.take(INDEX_MAX_BYTES + 1).read_to_end(&mut bytes) {
        return Err(damaged(format!("unreadable: {err}")));
    }
    if bytes.len() as u64 > INDEX_MAX_BYTES {
        return Err(damaged("too large"));
    }
    let text = String::from_utf8(bytes).map_err(|_| damaged("not UTF-8"))?;
    decode_index(&text, key)
}

/// Writes `index` to `path` the way every autosave lands: a unique,
/// owner-only temp file beside it, synced, then renamed over it. `false`
/// (logged, temp removed) on any failure, leaving the previous index.
pub(crate) fn write_index(path: &Path, index: &AutosaveIndex) -> bool {
    let temp = crate::autosave_temp_path(path);
    let Some(mut file) = crate::create_autosave_temp(&temp) else {
        return false;
    };
    let written = file
        .write_all(encode_index(index).as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written {
        tracing::warn!(?err, path = %temp.display(), "failed to write the autosave index");
        crate::remove_autosave_temp(&temp);
        return false;
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        tracing::warn!(?err, path = %path.display(), "failed to swap the autosave index into place");
        crate::remove_autosave_temp(&temp);
        return false;
    }
    true
}

/// The names in `dir` that start with `aurora-autosave` — only those are
/// copied (0.171.1, L-b): the temp directory can hold thousands of
/// unrelated files.
fn dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name();
                    let name = name.to_str()?;
                    name.starts_with(PREFIX).then(|| name.to_owned())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The index recovery uses when `key`'s own is missing or damaged: every
/// session file (canonical or partial) in `key`'s namespace, by id, the
/// lowest id active. Temp files and other runs' files are ignored.
pub(crate) fn scan_namespace(dir: &Path, key: impl Into<RunKey>) -> AutosaveIndex {
    let key = key.into();
    let ids: BTreeSet<u64> = dir_names(dir)
        .iter()
        .filter_map(|name| parse_session_file_name(name))
        .filter(|(file_key, _, _)| *file_key == key)
        .map(|(_, id, _)| id)
        .collect();
    AutosaveIndex {
        active: ids.first().copied(),
        entries: ids
            .into_iter()
            .map(|id| IndexEntry {
                id,
                file: session_file_name(key, id),
            })
            .collect(),
    }
}

/// One document recovery will try: a canonical session path (its partial
/// sibling is `crate::recover_document`'s business), and which crashed
/// run it belongs to (`None` for the legacy file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) path: PathBuf,
    pub(crate) source: Option<RunKey>,
    /// The session id within its run (`0` for the legacy file).
    pub(crate) id: u64,
}

/// What [`plan_recovery`] found.
#[derive(Debug, Default)]
pub(crate) struct RecoveryPlan {
    /// Every document to try, the first source's active one first.
    pub(crate) candidates: Vec<Candidate>,
    /// Sources whose index was damaged (and so were scanned instead).
    pub(crate) damaged_indexes: Vec<RunKey>,
}

/// The documents crashed runs left, from each one's index or, when that
/// is missing or damaged, a scan of its namespace; in `sources` order,
/// each path once. When no source names anything, the legacy single file
/// (or its partial sibling) from a pre-0.171.0 crash is the one candidate.
pub(crate) fn plan_recovery(dir: &Path, sources: &[RunKey]) -> RecoveryPlan {
    let mut plan = RecoveryPlan::default();
    for &key in sources {
        let index = match read_index(&index_path(dir, key), key) {
            Ok(index) => index,
            Err(IndexError::Missing) => scan_namespace(dir, key),
            Err(IndexError::Damaged(reason)) => {
                tracing::warn!(%key, %reason, "the autosave index is damaged; scanning that run's files instead");
                plan.damaged_indexes.push(key);
                scan_namespace(dir, key)
            }
        };
        for entry in index.recovery_order() {
            let path = dir.join(&entry.file);
            if !plan.candidates.iter().any(|known| known.path == path) {
                plan.candidates.push(Candidate {
                    path,
                    source: Some(key),
                    id: entry.id,
                });
            }
        }
    }
    let legacy = legacy_path(dir);
    if plan.candidates.is_empty()
        && (legacy.exists() || crate::partial_autosave_path(&legacy).exists())
    {
        plan.candidates.push(Candidate {
            path: legacy,
            source: None,
            id: 0,
        });
    }
    plan
}

/// The marker's text: the given keys, the writer's own first.
pub(crate) fn encode_marker(keys: &[RunKey]) -> String {
    let mut unique: Vec<String> = Vec::new();
    for key in keys {
        let key = key.to_string();
        if !unique.contains(&key) {
            unique.push(key);
        }
    }
    format!("{MARKER_MAGIC} 1\n{}\n", unique.join(" "))
}

/// The keys a marker records, writer first; empty for a pre-0.171.0
/// (empty), damaged or unreadable marker. Lenient: anything that is not
/// a key is skipped, repeats are dropped, and at most
/// [`MARKER_MAX_PARSED`] are taken (0.171.0 review L2).
pub(crate) fn decode_marker(text: &str) -> Vec<RunKey> {
    let mut seen = BTreeSet::new();
    let mut keys = Vec::new();
    for line in text.lines().filter(|line| !line.starts_with(MARKER_MAGIC)) {
        for key in line.split_whitespace().filter_map(RunKey::parse) {
            if keys.len() >= MARKER_MAX_PARSED {
                return keys;
            }
            if seen.insert(key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// The keys in the marker at `path` (at most [`MARKER_MAX_BYTES`] read);
/// `None` when there is no marker (the previous run shut down cleanly),
/// `Some(empty)` for a legacy or unreadable one.
pub(crate) fn read_marker(path: &Path) -> Option<Vec<RunKey>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(?err, path = %path.display(), "the session marker is unreadable");
            return Some(Vec::new());
        }
    };
    let mut bytes = Vec::new();
    if let Err(err) = file.take(MARKER_MAX_BYTES).read_to_end(&mut bytes) {
        tracing::warn!(?err, path = %path.display(), "the session marker is unreadable");
        return Some(Vec::new());
    }
    Some(decode_marker(&String::from_utf8_lossy(&bytes)))
}

/// Writes the marker the way the index lands (0.171.0 review N1): a
/// unique, owner-only temp file beside it, synced, then renamed over it,
/// so a crash or power loss mid-write leaves the previous marker whole,
/// never an empty or zero-filled one. The rename also replaces a symlink
/// planted at the marker's path instead of writing through it. `false`
/// (logged, temp removed) on any failure.
pub(crate) fn write_marker(path: &Path, text: &str) -> bool {
    let temp = crate::autosave_temp_path(path);
    let Some(mut file) = crate::create_autosave_temp(&temp) else {
        return false;
    };
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written {
        tracing::warn!(?err, path = %temp.display(), "failed to write the session marker");
        crate::remove_autosave_temp(&temp);
        return false;
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        tracing::warn!(?err, path = %path.display(), "failed to swap the session marker into place");
        crate::remove_autosave_temp(&temp);
        return false;
    }
    true
}

/// The run a file in the autosave directory belongs to, and whether it is
/// data (a session file, its partial or temp, the index or an index temp
/// file — 0.171.0 review N3) rather than the run's lock.
fn file_run_key(name: &str) -> Option<(RunKey, bool)> {
    let rest = name.strip_prefix(PREFIX)?.strip_prefix('-')?;
    if let Some(key) = rest.strip_suffix(".lock") {
        return RunKey::parse(key).map(|key| (key, false));
    }
    if let Some(key) = rest.strip_suffix(".index") {
        return RunKey::parse(key).map(|key| (key, true));
    }
    if let Some((key, tail)) = rest.split_once(".index.")
        && tail.rsplit('.').next() == Some("tmp")
        && let Some(key) = RunKey::parse(key)
    {
        return Some((key, true));
    }
    let (key, _) = rest.split_once('-')?;
    RunKey::parse(key).map(|key| (key, true))
}

/// Every run with data (an index, an index temp or session files) among
/// `names`.
fn keys_with_data(names: &[String]) -> BTreeSet<RunKey> {
    names
        .iter()
        .filter_map(|name| file_run_key(name))
        .filter(|&(_, data)| data)
        .map(|(key, _)| key)
        .collect()
}

/// `keys` with those that have data first (each group in its own order),
/// at most `cap` of them.
fn prioritised(
    keys: impl IntoIterator<Item = RunKey>,
    data: &BTreeSet<RunKey>,
    cap: usize,
) -> Vec<RunKey> {
    let (with, without): (Vec<RunKey>, Vec<RunKey>) =
        keys.into_iter().partition(|key| data.contains(key));
    with.into_iter().chain(without).take(cap).collect()
}

/// The runs to recover from, given a previous marker's keys (0.171.0
/// review N1/N2). An empty list — a pre-0.171.0 marker, or one that a
/// crash left empty, zero-filled or unparseable — falls back to scanning
/// the directory for every run with an index or session files, so a
/// damaged marker can never strand a crashed run's documents (a live
/// run among them is then skipped by [`claim_sources`]). Otherwise the
/// marker's keys, those with data first, at most [`MARKER_MAX_KEYS`].
///
/// Since 0.171.1 (L-c) the scan always runs: every run with data the
/// marker does not name is appended after the marker's own keys, so a
/// stale marker — a failed marker write leaves the old one — can order
/// recovery but never hide a crashed run.
pub(crate) fn recovery_keys(dir: &Path, previous: &[RunKey]) -> Vec<RunKey> {
    let data = keys_with_data(&dir_names(dir));
    let named: BTreeSet<RunKey> = previous.iter().copied().collect();
    let keys = previous
        .iter()
        .copied()
        .chain(data.iter().copied().filter(|key| !named.contains(key)));
    prioritised(keys, &data, MARKER_MAX_KEYS)
}

/// Opens (creating when `create`) the lock file at `path` and takes its
/// exclusive lock without waiting: `Some` while held, `None` when another
/// process holds it, it is missing (and not `create`) or it cannot be
/// opened.
pub(crate) fn try_lock(path: &Path, create: bool) -> Option<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(?err, path = %path.display(), "failed to open an autosave lock");
            }
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(err)) => {
            tracing::warn!(?err, path = %path.display(), "failed to take an autosave lock");
            None
        }
    }
}

/// A crashed run this run recovers from, with its lock held (when it had
/// one) so no other starting Aurora adopts it at the same time.
#[derive(Debug)]
pub(crate) struct RecoverySource {
    pub(crate) key: RunKey,
    _lock: Option<File>,
}

/// The marker's runs that are really gone: a run whose lock file is
/// missing (a live run creates its own before writing anything) or whose
/// lock this process can take. A run whose lock is held is alive — a
/// second Aurora — and is skipped, so its files are never read, adopted
/// or deleted. `own` is never a source: its namespace was chosen empty
/// ([`AutosaveNamespace::acquire`]).
///
/// A missing lock file is not proof of death on its own (0.171.1, L-a):
/// a temp cleaner may have removed a live run's. If the run's index or
/// session files were modified within [`LIVENESS_WINDOW`] it is treated
/// as possibly alive and skipped this time.
pub(crate) fn claim_sources(dir: &Path, own: RunKey, marker: &[RunKey]) -> Vec<RecoverySource> {
    claim_sources_at(dir, own, marker, SystemTime::now())
}

/// [`claim_sources`] at a given `now`, for tests.
pub(crate) fn claim_sources_at(
    dir: &Path,
    own: RunKey,
    marker: &[RunKey],
    now: SystemTime,
) -> Vec<RecoverySource> {
    let names = dir_names(dir);
    let mut sources = Vec::new();
    for &key in marker.iter().filter(|&&key| key != own) {
        let path = lock_path(dir, key);
        if !path.exists() {
            if recently_modified(dir, key, &names, now) {
                tracing::info!(%key, "a run without a lock file has recent files; treated as possibly alive");
            } else {
                sources.push(RecoverySource { key, _lock: None });
            }
            continue;
        }
        if let Some(lock) = try_lock(&path, false) {
            sources.push(RecoverySource {
                key,
                _lock: Some(lock),
            });
        } else {
            tracing::info!(%key, "another Aurora is still running; its autosaves are left alone");
        }
    }
    // A gone run with nothing to recover — only index temp files, say from
    // a thread detached at exit — is retired here, silently: it is no
    // source, so it cannot show a crash dialog that recovers nothing
    // (0.171.1 review R-2). Its lock, when it had one, is held right now.
    sources.retain(|source| {
        let recoverable = names.iter().any(|name| {
            file_run_key(name) == Some((source.key, true)) && !is_index_temp(name)
        });
        if !recoverable {
            tracing::info!(key = %source.key, "a gone run left nothing to recover; its leftovers are removed");
            retire_namespace(dir, source.key);
        }
        recoverable
    });
    sources
}

/// Whether `name` is an index temp file (`aurora-autosave-<key>.index.*.tmp`).
fn is_index_temp(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.split_once(".index."))
        .is_some_and(|(_, tail)| tail.rsplit('.').next() == Some("tmp"))
}

/// Whether any of run `key`'s data files among `names` was modified
/// within [`LIVENESS_WINDOW`] of `now`, either side: a modification time
/// far in the future (clock skew, or a hostile file) counts as old, so it
/// can never keep a crashed run from recovery forever. An unreadable time
/// counts as old for the same reason.
fn recently_modified(dir: &Path, key: RunKey, names: &[String], now: SystemTime) -> bool {
    names
        .iter()
        .filter(|name| file_run_key(name) == Some((key, true)))
        .filter_map(|name| std::fs::metadata(dir.join(name)).ok()?.modified().ok())
        .any(|modified| {
            let distance = match now.duration_since(modified) {
                Ok(age) => age,
                Err(ahead) => ahead.duration(),
            };
            distance < LIVENESS_WINDOW
        })
}

/// Whether anything of run `key` is among `names`: its lock (alive, or
/// crashed and not yet retired), its index or an index temp file, or a
/// session file.
fn has_files(key: RunKey, names: &[String]) -> bool {
    names
        .iter()
        .any(|name| file_run_key(name).is_some_and(|(file_key, _)| file_key == key))
}

/// The keys a new marker inherits from `previous`: every run other than
/// `own` that still has anything on disk, so a crash *during* recovery
/// still finds the older files and a retired run drops out; those with
/// data first, at most one fewer than [`MARKER_MAX_KEYS`] (the writer
/// takes the first place).
pub(crate) fn inherited_marker_keys(dir: &Path, own: RunKey, previous: &[RunKey]) -> Vec<RunKey> {
    let names = dir_names(dir);
    let data = keys_with_data(&names);
    let present: BTreeSet<RunKey> = names
        .iter()
        .filter_map(|name| file_run_key(name))
        .map(|(key, _)| key)
        .collect();
    prioritised(
        previous
            .iter()
            .copied()
            .filter(|&key| key != own && present.contains(&key)),
        &data,
        MARKER_MAX_KEYS - 1,
    )
}

/// What a clean quit leaves in the marker (0.171.0 review M1): the runs
/// it lists, minus `own` (whose namespace is already gone), that still
/// have files — a crashed run whose other documents were kept for a
/// later round, or a second Aurora still running. Empty means the marker
/// can be deleted.
///
/// Since the 0.171.1 review (L-2) the directory scan is added too: a run
/// another run's stale marker never named (its own marker write failed)
/// still has files, so it must stay in the marker this quit leaves —
/// otherwise this quit could delete the marker and strand that run when
/// it later crashes.
pub(crate) fn marker_after_quit(dir: &Path, own: RunKey, marker: &[RunKey]) -> Vec<RunKey> {
    let data = keys_with_data(&dir_names(dir));
    let named: BTreeSet<RunKey> = marker.iter().copied().collect();
    let keys: Vec<RunKey> = marker
        .iter()
        .copied()
        .chain(data.into_iter().filter(|key| !named.contains(key)))
        .collect();
    inherited_marker_keys(dir, own, &keys)
}

/// Where startup recovers from (0.171.1 review L-2): the runs to try
/// ([`recovery_keys`]: the marker's keys plus the directory scan — the
/// scan runs even when there is **no marker at all**, so a missing marker
/// can never strand a crashed run), the ones claimed as really gone
/// ([`claim_sources`]), and whether this start is a crash recovery: a
/// marker naming a gone run, a legacy (empty) marker, or — with no marker
/// — any gone run with files the scan found.
#[derive(Debug)]
pub(crate) struct StartupSources {
    pub(crate) keys: Vec<RunKey>,
    pub(crate) claimed: Vec<RecoverySource>,
    pub(crate) had_previous_marker: bool,
}

/// [`StartupSources`] for this run (`own`) and the previous marker.
pub(crate) fn startup_sources(
    dir: &Path,
    own: RunKey,
    previous: Option<&[RunKey]>,
) -> StartupSources {
    let keys = recovery_keys(dir, previous.unwrap_or_default());
    let claimed = claim_sources(dir, own, &keys);
    let had_previous_marker = match previous {
        Some(marker) => marker.is_empty() || !claimed.is_empty(),
        None => !claimed.is_empty(),
    };
    StartupSources {
        keys,
        claimed,
        had_previous_marker,
    }
}

fn remove_quietly(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?err, path = %path.display(), "failed to remove an autosave file");
    }
}

/// Deletes `key`'s data files in `dir` except its index: sessions,
/// partials, their temps, and index temp files (review N3). Only names
/// belonging to `key` itself are touched.
fn remove_session_files(dir: &Path, key: RunKey) {
    let index = index_path(dir, key);
    for name in dir_names(dir) {
        let path = dir.join(&name);
        if path != index && file_run_key(&name) == Some((key, true)) {
            remove_quietly(&path);
        }
    }
}

/// Retires a crashed run once nothing of it is still needed: its session
/// files, then its index, then its lock file. The caller holds the lock
/// (a [`RecoverySource`]) or the lock file was missing.
pub(crate) fn retire_namespace(dir: &Path, key: RunKey) {
    remove_session_files(dir, key);
    remove_quietly(&index_path(dir, key));
    remove_quietly(&lock_path(dir, key));
}

/// Rewrites a crashed run's index to list only `remaining` — the
/// sessions recovery has not tried yet — after one was adopted out of it,
/// so the next recovery neither reports the moved file as missing nor
/// loses the rest. The caller holds the run's lock.
pub(crate) fn rewrite_remaining(dir: &Path, key: RunKey, remaining: &[u64]) -> bool {
    let entries: Vec<IndexEntry> = remaining
        .iter()
        .map(|&id| IndexEntry {
            id,
            file: session_file_name(key, id),
        })
        .collect();
    write_index(
        &index_path(dir, key),
        &AutosaveIndex {
            active: entries.first().map(|entry| entry.id),
            entries,
        },
    )
}

/// Retires the legacy single file and its partial sibling.
pub(crate) fn retire_legacy(dir: &Path) {
    let legacy = legacy_path(dir);
    remove_quietly(&legacy);
    remove_quietly(&crate::partial_autosave_path(&legacy));
}

/// Moves a recovered session's file (and its partial sibling) to `to`, so
/// the recovering run owns it under its own key and id without
/// rewriting it. `true` when whatever existed moved. Same directory, so
/// each move is one atomic rename.
pub(crate) fn adopt(from: &Path, to: &Path) -> bool {
    let mut moved = false;
    for (source, target) in [
        (from.to_path_buf(), to.to_path_buf()),
        (
            crate::partial_autosave_path(from),
            crate::partial_autosave_path(to),
        ),
    ] {
        if !source.exists() {
            continue;
        }
        if let Err(err) = std::fs::rename(&source, &target) {
            tracing::warn!(?err, path = %source.display(), "failed to adopt a recovered autosave");
            return false;
        }
        moved = true;
    }
    moved
}

/// The proof that a run is alive (0.171.1, L-a): its exclusive lock,
/// re-created if a temp cleaner removed the file, and its lock file's and
/// index's modification times, bumped by [`Self::refresh`] on every
/// autosave landing and every [`LIVENESS_REFRESH`] on the autosave
/// thread. Shared by the run's [`AutosaveNamespace`] and its worker.
#[derive(Debug)]
pub(crate) struct Liveness {
    dir: PathBuf,
    key: RunKey,
    /// The held lock; `None` when it could not be taken.
    lock: Mutex<Option<File>>,
    /// Set once by [`Self::release`] (a clean quit), under `lock`, so no
    /// refresh can re-create the lock file after it is deleted.
    released: AtomicBool,
    /// The session marker, refreshed with the rest (0.172.0).
    marker: Mutex<Option<PathBuf>>,
}

impl Liveness {
    fn held(&self) -> std::sync::MutexGuard<'_, Option<File>> {
        self.lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Re-creates and re-takes the lock if its file is gone, then bumps
    /// the lock file's and the index's modification times. `true` while
    /// the lock is held. A no-op after [`Self::release`].
    pub(crate) fn refresh(&self) -> bool {
        let mut held = self.held();
        if self.released.load(Ordering::SeqCst) {
            return false;
        }
        let path = lock_path(&self.dir, self.key);
        if held.is_none() || !path.exists() {
            if let Some(fresh) = try_lock(&path, true) {
                *held = Some(fresh);
            } else {
                tracing::warn!(key = %self.key, "could not re-create this run's autosave lock");
            }
        }
        let now = SystemTime::now();
        if let Some(file) = held.as_ref()
            && let Err(err) = file.set_modified(now)
        {
            tracing::debug!(?err, "could not refresh the autosave lock's time");
        }
        if let Ok(index) = std::fs::OpenOptions::new()
            .write(true)
            .open(index_path(&self.dir, self.key))
            && let Err(err) = index.set_modified(now)
        {
            tracing::debug!(?err, "could not refresh the autosave index's time");
        }
        // 0.172.0: this run's own session files and the marker too, so a
        // temp cleaner cannot age out an idle (parked) document's autosave.
        for name in dir_names(&self.dir) {
            if parse_session_file_name(&name).is_some_and(|(key, _, _)| key == self.key) {
                touch(&self.dir.join(&name), now);
            }
        }
        let marker = self
            .marker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(marker) = marker {
            touch(&marker, now);
        }
        held.is_some()
    }

    /// Names the session marker [`Self::refresh`] keeps fresh (0.172.0).
    pub(crate) fn watch_marker(&self, path: PathBuf) {
        *self
            .marker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path);
    }

    /// Releases the lock for good (a clean quit): no later refresh does
    /// anything.
    pub(crate) fn release(&self) {
        let mut held = self.held();
        self.released.store(true, Ordering::SeqCst);
        drop(held.take());
    }
}

/// Bumps a file's modification time without changing it (0.172.0).
fn touch(path: &Path, now: SystemTime) {
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path)
        && let Err(err) = file.set_modified(now)
    {
        tracing::debug!(?err, path = %path.display(), "could not refresh a file's time");
    }
}

/// This run's own corner of the autosave directory: its key and the lock
/// that tells other runs it is alive ([`Liveness`]).
#[derive(Debug)]
pub(crate) struct AutosaveNamespace {
    pub(crate) dir: PathBuf,
    pub(crate) key: RunKey,
    liveness: Arc<Liveness>,
}

impl AutosaveNamespace {
    /// Picks this run's key — `pid`, or the first `pid.<generation>` with
    /// nothing on disk when a crashed run with the same pid left files
    /// (0.171.0 review M1) — and takes its lock (logged, not fatal, if it
    /// cannot: the run then looks crashed to a second Aurora, which is
    /// the pre-0.171.0 behaviour; [`Liveness::refresh`] tries again).
    pub(crate) fn acquire(dir: PathBuf, pid: u32) -> Self {
        let names = dir_names(&dir);
        let key = (0..MAX_GENERATIONS)
            .map(|generation| RunKey { pid, generation })
            .find(|&key| !has_files(key, &names))
            .unwrap_or_else(|| {
                tracing::warn!(pid, "every autosave generation of this pid is taken");
                RunKey {
                    pid,
                    generation: MAX_GENERATIONS,
                }
            });
        let lock = try_lock(&lock_path(&dir, key), true);
        if lock.is_none() {
            tracing::warn!(%key, "could not take this run's autosave lock");
        }
        let liveness = Arc::new(Liveness {
            dir: dir.clone(),
            key,
            lock: Mutex::new(lock),
            released: AtomicBool::new(false),
            marker: Mutex::new(None),
        });
        Self { dir, key, liveness }
    }

    /// The run's liveness, for its autosave worker to refresh.
    pub(crate) fn liveness(&self) -> Arc<Liveness> {
        Arc::clone(&self.liveness)
    }

    /// Whether this run holds its lock.
    #[cfg(test)]
    pub(crate) fn locked(&self) -> bool {
        self.liveness.held().is_some()
    }

    /// Where session `id`'s autosave lives.
    pub(crate) fn session_path(&self, id: u64) -> PathBuf {
        self.dir.join(session_file_name(self.key, id))
    }

    /// This run's index.
    pub(crate) fn index_path(&self) -> PathBuf {
        index_path(&self.dir, self.key)
    }

    /// A clean quit's cleanup: the lock is released for good (so no
    /// refresh can re-create it), then every file in this run's namespace,
    /// its index and its lock file are removed. Only this run's key is
    /// touched — never a crashed run's, even one with the same pid (it
    /// has another generation). Idempotent.
    pub(crate) fn remove_all(&mut self) {
        self.liveness.release();
        remove_session_files(&self.dir, self.key);
        remove_quietly(&self.index_path());
        remove_quietly(&lock_path(&self.dir, self.key));
    }
}

/// One index write (0.171.1, L3): its listing is computed under the
/// autosave worker's state lock ([`IndexBook::prepare`]), its temp file
/// written and synced outside it ([`write_index_temp`]), and it is
/// renamed into place back under the lock by [`IndexBook::finish`] only
/// if no newer write has landed — so a slow fsync never holds the lock,
/// and an older listing can never overwrite a newer one.
#[derive(Debug)]
#[must_use = "a prepared index write must be finished or abandoned"]
pub(crate) struct IndexWrite {
    seq: u64,
    path: PathBuf,
    listing: AutosaveIndex,
    /// Files of removed sessions, deleted once this index has landed.
    then_remove: Vec<PathBuf>,
}

/// Writes a prepared listing to a synced temp file beside its index —
/// the slow half of [`IndexWrite`], run outside any lock.
pub(crate) fn write_index_temp(write: &IndexWrite) -> Option<PathBuf> {
    let temp = crate::autosave_temp_path(&write.path);
    let mut file = crate::create_autosave_temp(&temp)?;
    let written = file
        .write_all(encode_index(&write.listing).as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written {
        tracing::warn!(?err, path = %temp.display(), "failed to write the autosave index");
        crate::remove_autosave_temp(&temp);
        return None;
    }
    Some(temp)
}

/// Prepares, writes and finishes one index write on the calling thread —
/// for a book no other thread shares (tests).
#[cfg(test)]
pub(crate) fn apply_index_write(book: &mut IndexBook, write: Option<IndexWrite>) -> bool {
    match write {
        None => true,
        Some(write) => {
            let temp = write_index_temp(&write);
            book.finish(write, temp)
        }
    }
}

/// What the run's index should list, kept in step with the files that
/// have landed (`crate::background_autosave` owns one). The index is
/// rewritten only when its listing changes — a session created, removed
/// or landing complete for the first time, or the active one changing —
/// never on an autosave that only replaces an already listed file.
#[derive(Debug)]
pub(crate) struct IndexBook {
    path: PathBuf,
    order: Vec<IndexEntry>,
    active: Option<u64>,
    complete: BTreeSet<u64>,
    /// What the index on disk lists, and the write that put it there.
    renamed: Option<AutosaveIndex>,
    renamed_seq: u64,
    next_seq: u64,
    /// Removed sessions' files, deleted once an index without them lands.
    pending_remove: Vec<PathBuf>,
    /// Writes prepared and not yet finished.
    in_flight: usize,
    writes: usize,
}

impl IndexBook {
    /// A book for the index at `path`, over `order` (tab order) with
    /// `active`, and the sessions in `complete` already landed.
    pub(crate) fn new(
        path: PathBuf,
        order: Vec<IndexEntry>,
        active: Option<u64>,
        complete: impl IntoIterator<Item = u64>,
    ) -> Self {
        Self {
            path,
            order,
            active,
            complete: complete.into_iter().collect(),
            renamed: None,
            renamed_seq: 0,
            next_seq: 0,
            pending_remove: Vec::new(),
            in_flight: 0,
            writes: 0,
        }
    }

    /// The index as it must read now: only complete sessions, in order;
    /// the active one if complete, else the first listed.
    pub(crate) fn listing(&self) -> AutosaveIndex {
        let entries: Vec<IndexEntry> = self
            .order
            .iter()
            .filter(|entry| self.complete.contains(&entry.id))
            .cloned()
            .collect();
        let active = self
            .active
            .filter(|id| entries.iter().any(|entry| entry.id == *id))
            .or_else(|| entries.first().map(|entry| entry.id));
        AutosaveIndex { active, entries }
    }

    /// The write that brings the index on disk up to [`Self::listing`],
    /// numbered after every earlier one; `None` when it already matches
    /// (or there is still nothing to list) and nothing awaits removal.
    pub(crate) fn prepare(&mut self) -> Option<IndexWrite> {
        let listing = self.listing();
        let current = self.renamed.as_ref() == Some(&listing)
            || (self.renamed.is_none() && listing.entries.is_empty());
        if current && self.pending_remove.is_empty() {
            return None;
        }
        self.next_seq += 1;
        self.in_flight += 1;
        Some(IndexWrite {
            seq: self.next_seq,
            path: self.path.clone(),
            listing,
            then_remove: std::mem::take(&mut self.pending_remove),
        })
    }

    /// Lands a prepared write's synced temp file (`None`: writing it
    /// failed): renamed over the index only if no newer write has landed,
    /// else discarded. `true` when the index on disk is at least as new as
    /// this write. Removed sessions' files go once their index has landed.
    pub(crate) fn finish(&mut self, write: IndexWrite, temp: Option<PathBuf>) -> bool {
        self.in_flight = self.in_flight.saturating_sub(1);
        let Some(temp) = temp else {
            self.pending_remove.extend(write.then_remove);
            return false;
        };
        let landed = if write.seq <= self.renamed_seq {
            crate::remove_autosave_temp(&temp);
            true
        } else if let Err(err) = std::fs::rename(&temp, &write.path) {
            tracing::warn!(?err, path = %write.path.display(), "failed to swap the autosave index into place");
            crate::remove_autosave_temp(&temp);
            false
        } else {
            self.renamed = Some(write.listing);
            self.renamed_seq = write.seq;
            self.writes += 1;
            true
        };
        if landed {
            for file in write.then_remove {
                remove_quietly(&file);
            }
        } else {
            self.pending_remove.extend(write.then_remove);
        }
        landed
    }

    /// The write that catches the index on disk up with the book when no
    /// other write is in flight (0.171.1 review L-1): after a failed write,
    /// or after an older write landed last while the newer one failed, the
    /// disk can lag the book until something else changes; the worker
    /// calls this after every finish and on its idle timer.
    pub(crate) fn retry_write(&mut self) -> Option<IndexWrite> {
        if self.in_flight == 0 {
            self.prepare()
        } else {
            None
        }
    }

    /// Hands back a prepared write that was never carried out — cancelled,
    /// refused, or dropped after the last attempt (0.171.1 review R-1):
    /// it no longer counts as in flight, and the removals it carried wait
    /// for the next write.
    pub(crate) fn abandon(&mut self, write: IndexWrite) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.pending_remove.extend(write.then_remove);
    }

    /// How many writes are prepared and not yet finished or abandoned.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Session `id`'s file landed complete: the index write that adds it,
    /// if any.
    pub(crate) fn mark_complete(&mut self, id: u64) -> Option<IndexWrite> {
        self.complete.insert(id);
        self.prepare()
    }

    /// Replaces the session set (creation, closing) and the active one:
    /// the index write without the removed sessions, whose files are
    /// deleted once it has landed — never before, so a crash in between
    /// still has them.
    pub(crate) fn set_sessions(
        &mut self,
        order: Vec<IndexEntry>,
        active: Option<u64>,
    ) -> Option<IndexWrite> {
        let removed: Vec<IndexEntry> = self
            .order
            .iter()
            .filter(|old| !order.iter().any(|new| new.id == old.id))
            .cloned()
            .collect();
        self.order = order;
        self.active = active;
        if let Some(dir) = self.path.parent() {
            for entry in &removed {
                let file = dir.join(&entry.file);
                self.pending_remove
                    .push(crate::partial_autosave_path(&file));
                self.pending_remove.push(file);
            }
        }
        for entry in &removed {
            self.complete.remove(&entry.id);
        }
        self.prepare()
    }

    /// How many times the index file was written.
    #[cfg(test)]
    pub(crate) fn writes(&self) -> usize {
        self.writes
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{IndexWrite, LIVENESS_WINDOW, apply_index_write, claim_sources_at, dir_names};
    use super::{inherited_marker_keys, recovery_keys, retire_namespace, write_marker};
    use super::{marker_after_quit as after_quit, startup_sources};

    fn sync(book: &mut IndexBook) -> bool {
        let write = book.prepare();
        apply_index_write(book, write)
    }

    fn mark(book: &mut IndexBook, id: u64) -> bool {
        let write = book.mark_complete(id);
        apply_index_write(book, write)
    }

    fn set(book: &mut IndexBook, order: Vec<IndexEntry>, active: Option<u64>) -> bool {
        let write = book.set_sessions(order, active);
        apply_index_write(book, write)
    }

    /// Every name in `dir`, not only Aurora's.
    fn all_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn set_mtime(path: &Path, time: std::time::SystemTime) {
        match std::fs::OpenOptions::new().write(true).open(path) {
            Ok(file) => assert!(file.set_modified(time).is_ok()),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    use super::{
        AutosaveIndex, AutosaveNamespace, IndexBook, IndexEntry, IndexError, RunKey, claim_sources,
        decode_index, decode_marker, encode_index, encode_marker, index_path, legacy_path,
        lock_path, marker_after_quit, parse_session_file_name, plan_recovery, read_index,
        read_marker, scan_namespace, session_file_name, try_lock, write_index,
    };

    fn tempdir() -> tempfile::TempDir {
        match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn touch(path: &Path) {
        if let Err(err) = std::fs::write(path, b"x") {
            unreachable!("{err:?}");
        }
    }

    fn key(pid: u32) -> RunKey {
        RunKey::from(pid)
    }

    fn entry(pid: u32, id: u64) -> IndexEntry {
        IndexEntry {
            id,
            file: session_file_name(pid, id),
        }
    }

    fn two_sessions() -> AutosaveIndex {
        AutosaveIndex {
            active: Some(7),
            entries: vec![entry(42, 3), entry(42, 7)],
        }
    }

    #[test]
    fn run_keys_have_exactly_one_spelling() {
        assert_eq!(RunKey::parse("12"), Some(key(12)));
        assert_eq!(
            RunKey::parse("12.3"),
            Some(RunKey {
                pid: 12,
                generation: 3
            })
        );
        for bad in [
            "", "12.0", "012", "+12", "12.", ".3", "12.03", "1 2", "12.3.4",
        ] {
            assert_eq!(RunKey::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn session_file_names_parse_and_reject_everything_else() {
        assert_eq!(
            parse_session_file_name("aurora-autosave-42-7.aur"),
            Some((key(42), 7, false))
        );
        assert_eq!(
            parse_session_file_name("aurora-autosave-42.2-7.partial.aur"),
            Some((
                RunKey {
                    pid: 42,
                    generation: 2
                },
                7,
                true
            ))
        );
        for bad in [
            "aurora-autosave.aur",
            "aurora-autosave-42.index",
            "aurora-autosave-42.lock",
            "aurora-autosave-42-7.aur.42.0.00ff.tmp",
            "aurora-autosave-+4-7.aur",
            "aurora-autosave-42.0-7.aur",
            "aurora-autosave-42-../x.aur",
            "aurora-autosave-42-7",
        ] {
            assert_eq!(parse_session_file_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_index_round_trips_through_its_text() {
        let index = two_sessions();
        assert_eq!(decode_index(&encode_index(&index), key(42)), Ok(index));
        let empty = AutosaveIndex::default();
        assert_eq!(decode_index(&encode_index(&empty), key(42)), Ok(empty));
    }

    #[test]
    fn an_index_naming_another_runs_file_is_damaged() {
        // 0.171.0 review L1: a damaged or hostile index for run 42 must
        // not make recovery adopt (and delete) live run 43's file.
        let foreign = AutosaveIndex {
            active: Some(1),
            entries: vec![entry(43, 1)],
        };
        assert!(matches!(
            decode_index(&encode_index(&foreign), key(42)),
            Err(IndexError::Damaged(_))
        ));
        let dir = tempdir();
        assert!(write_index(&index_path(dir.path(), 42), &foreign));
        touch(&dir.path().join(session_file_name(43, 1)));
        let plan = plan_recovery(dir.path(), &[key(42)]);
        assert!(plan.candidates.is_empty(), "{:?}", plan.candidates);
        assert_eq!(plan.damaged_indexes, vec![key(42)]);
    }

    #[test]
    fn a_truncated_or_garbage_index_is_damaged_never_read_short() {
        let text = encode_index(&two_sessions());
        // Every proper prefix is damaged: none reads as a shorter list.
        for cut in 0..text.len() - 1 {
            let Some(prefix) = text.get(..cut) else {
                continue;
            };
            assert!(
                matches!(decode_index(prefix, key(42)), Err(IndexError::Damaged(_))),
                "prefix of {cut} bytes read as an index"
            );
        }
        for garbage in [
            "",
            "\0\0\0\0",
            "aurora-autosave-index 2\nactive -\nend\n",
            "aurora-autosave-index 1\nactive 9\nend\n",
            "aurora-autosave-index 1\nactive -\nsession 1 ../../etc/passwd\nend\n",
            "aurora-autosave-index 1\nactive -\nsession 1 aurora-autosave-42-2.aur\nend\n",
            "aurora-autosave-index 1\nactive -\nsession 1 aurora-autosave-42-1.aur\nsession 1 aurora-autosave-42-1.aur\nend\n",
            "aurora-autosave-index 1\nactive -\nend\nsession 1 aurora-autosave-42-1.aur\n",
        ] {
            assert!(
                matches!(decode_index(garbage, key(42)), Err(IndexError::Damaged(_))),
                "{garbage:?} read as an index"
            );
        }
    }

    #[test]
    fn reading_a_missing_garbage_or_oversized_index_is_reported() {
        let dir = tempdir();
        let path = dir.path().join("x.index");
        assert_eq!(read_index(&path, key(42)), Err(IndexError::Missing));
        if let Err(err) = std::fs::write(&path, [0xff, 0xfe, 0x00, 0x13]) {
            unreachable!("{err:?}");
        }
        assert!(matches!(
            read_index(&path, key(42)),
            Err(IndexError::Damaged(_))
        ));
        if let Err(err) = std::fs::write(&path, vec![b'a'; (1 << 20) + 1]) {
            unreachable!("{err:?}");
        }
        assert!(matches!(
            read_index(&path, key(42)),
            Err(IndexError::Damaged(_))
        ));
        assert!(write_index(&path, &two_sessions()));
        assert_eq!(read_index(&path, key(42)), Ok(two_sessions()));
        // No temp file is left beside it.
        let leftovers = std::fs::read_dir(dir.path())
            .map(|entries| entries.flatten().count())
            .unwrap_or_default();
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn recovery_order_puts_the_active_session_first() {
        let order: Vec<u64> = two_sessions()
            .recovery_order()
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(order, vec![7, 3]);
    }

    #[test]
    fn a_scan_finds_only_that_runs_session_files() {
        let dir = tempdir();
        for name in [
            "aurora-autosave-42-5.aur",
            "aurora-autosave-42-2.partial.aur",
            "aurora-autosave-42-9.aur.42.0.00ff.tmp",
            "aurora-autosave-42.1-3.aur",
            "aurora-autosave-421-1.aur",
            "aurora-autosave-4-1.aur",
            "aurora-autosave.aur",
            "aurora-autosave-42.index",
        ] {
            touch(&dir.path().join(name));
        }
        let scanned = scan_namespace(dir.path(), 42);
        assert_eq!(
            scanned,
            AutosaveIndex {
                active: Some(2),
                entries: vec![entry(42, 2), entry(42, 5)],
            }
        );
    }

    #[test]
    fn the_plan_reads_the_index_and_ignores_unlisted_files() {
        let dir = tempdir();
        let index = AutosaveIndex {
            active: Some(1),
            entries: vec![entry(42, 1)],
        };
        assert!(write_index(&index_path(dir.path(), 42), &index));
        // A file that landed after the index was last written (a crash
        // between the file and the index) is not in the plan: the index
        // is authoritative while it reads.
        touch(&dir.path().join(session_file_name(42, 2)));
        let plan = plan_recovery(dir.path(), &[key(42)]);
        let paths: Vec<_> = plan.candidates.iter().map(|c| c.path.clone()).collect();
        assert_eq!(paths, vec![dir.path().join(session_file_name(42, 1))]);
        assert!(plan.damaged_indexes.is_empty());
    }

    #[test]
    fn a_damaged_or_missing_index_falls_back_to_scanning_that_run() {
        for damage in [
            Some(&b"aurora-autosave-index 1\nactive 1\nsess"[..]),
            Some(&b"\x00garbage"[..]),
            None,
        ] {
            let dir = tempdir();
            if let Some(bytes) = damage
                && let Err(err) = std::fs::write(index_path(dir.path(), 42), bytes)
            {
                unreachable!("{err:?}");
            }
            touch(&dir.path().join(session_file_name(42, 4)));
            touch(&dir.path().join(session_file_name(43, 1)));
            let plan = plan_recovery(dir.path(), &[key(42)]);
            let paths: Vec<_> = plan.candidates.iter().map(|c| c.path.clone()).collect();
            assert_eq!(
                paths,
                vec![dir.path().join(session_file_name(42, 4))],
                "{damage:?}"
            );
            assert_eq!(plan.damaged_indexes.len(), usize::from(damage.is_some()));
        }
    }

    #[test]
    fn the_legacy_file_is_the_candidate_only_when_no_source_names_anything() {
        let dir = tempdir();
        touch(&legacy_path(dir.path()));
        let plan = plan_recovery(dir.path(), &[]);
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(
            plan.candidates.first().map(|c| (c.path.clone(), c.source)),
            Some((legacy_path(dir.path()), None))
        );
        // A source that names a session wins over the legacy file.
        touch(&dir.path().join(session_file_name(42, 1)));
        let plan = plan_recovery(dir.path(), &[key(42)]);
        assert_eq!(
            plan.candidates.first().map(|c| c.source),
            Some(Some(key(42)))
        );
        assert_eq!(plan.candidates.len(), 1);
    }

    #[test]
    fn several_sources_are_merged_in_order_without_repeats() {
        let dir = tempdir();
        let newer = AutosaveIndex {
            active: Some(9),
            entries: vec![entry(50, 9)],
        };
        assert!(write_index(&index_path(dir.path(), 50), &newer));
        touch(&dir.path().join(session_file_name(40, 2)));
        let plan = plan_recovery(dir.path(), &[key(50), key(40), key(50)]);
        let sources: Vec<_> = plan.candidates.iter().map(|c| c.source).collect();
        assert_eq!(sources, vec![Some(key(50)), Some(key(40))]);
    }

    #[test]
    fn the_marker_records_the_writer_then_the_runs_it_inherits() {
        let reused = RunKey {
            pid: 7,
            generation: 2,
        };
        assert_eq!(
            decode_marker(&encode_marker(&[key(10), reused, key(10), key(3)])),
            vec![key(10), reused, key(3)]
        );
        assert_eq!(decode_marker(""), Vec::<RunKey>::new());
        assert_eq!(decode_marker("\u{fffd}junk 12 x-4 12 12.0"), vec![key(12)]);
    }

    #[test]
    fn a_huge_hostile_marker_is_read_bounded() {
        // 0.171.0 review L2: the temp directory is shared, so the marker
        // can be anything; reading it is bounded in bytes and keys.
        let dir = tempdir();
        let path = dir.path().join("aurora-session.marker");
        let text = (0..200_000_u32)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        if let Err(err) = std::fs::write(&path, text) {
            unreachable!("{err:?}");
        }
        let started = std::time::Instant::now();
        let keys = read_marker(&path).unwrap_or_default();
        assert!(!keys.is_empty() && keys.len() <= super::MARKER_MAX_PARSED);
        assert_eq!(
            recovery_keys(dir.path(), &keys).len(),
            super::MARKER_MAX_KEYS
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(read_marker(&dir.path().join("none")), None);
    }

    #[test]
    fn an_empty_or_zero_filled_marker_never_strands_a_crashed_run() {
        // 0.171.0 review N1: a power loss can leave the marker empty or
        // zero-filled; the directory scan still finds the crashed run.
        for bytes in [Vec::new(), vec![0_u8; 4096]] {
            let dir = tempdir();
            let marker = dir.path().join("aurora-session.marker");
            if let Err(err) = std::fs::write(&marker, &bytes) {
                unreachable!("{err:?}");
            }
            touch(&lock_path(dir.path(), 61));
            touch(&dir.path().join(session_file_name(61, 1)));
            // A run with only a lock file has nothing to recover.
            touch(&lock_path(dir.path(), 62));
            let previous = read_marker(&marker);
            assert_eq!(previous, Some(Vec::new()));
            let keys = recovery_keys(dir.path(), &previous.unwrap_or_default());
            assert_eq!(keys, vec![key(61)]);
            let claimed = claim_sources(dir.path(), key(63), &keys);
            assert_eq!(
                claimed.iter().map(|source| source.key).collect::<Vec<_>>(),
                vec![key(61)]
            );
            let plan = plan_recovery(dir.path(), &[key(61)]);
            assert_eq!(plan.candidates.len(), 1);
            // And the rewritten marker inherits it.
            assert_eq!(
                inherited_marker_keys(dir.path(), key(63), &keys),
                vec![key(61)]
            );
        }
    }

    #[test]
    fn the_marker_cap_keeps_runs_with_files_ahead_of_lock_only_ones() {
        // 0.171.0 review N2.
        let dir = tempdir();
        touch(&dir.path().join(session_file_name(99_999, 1)));
        let mut previous: Vec<RunKey> = (1..=1500).map(key).collect();
        previous.push(key(99_999));
        let keys = recovery_keys(dir.path(), &previous);
        assert_eq!(keys.len(), super::MARKER_MAX_KEYS);
        assert_eq!(keys.first(), Some(&key(99_999)));
        for pid in 1..=1500 {
            touch(&lock_path(dir.path(), pid));
        }
        let inherited = inherited_marker_keys(dir.path(), key(1), &previous);
        assert_eq!(inherited.len(), super::MARKER_MAX_KEYS - 1);
        assert_eq!(inherited.first(), Some(&key(99_999)));
    }

    #[test]
    fn an_index_temp_file_counts_as_files_and_is_retired() {
        // 0.171.0 review N3.
        let dir = tempdir();
        let temp = dir.path().join("aurora-autosave-70.index.70.0.00ff.tmp");
        touch(&temp);
        assert_eq!(
            inherited_marker_keys(dir.path(), key(71), &[key(70)]),
            vec![key(70)]
        );
        assert_eq!(recovery_keys(dir.path(), &[]), vec![key(70)]);
        let reused = AutosaveNamespace::acquire(dir.path().to_path_buf(), 70);
        assert_eq!(reused.key.generation, 1);
        drop(reused);
        retire_namespace(dir.path(), key(70));
        assert!(!temp.exists());
    }

    #[test]
    fn the_marker_is_written_atomically_and_leaves_no_temp_file() {
        let dir = tempdir();
        let path = dir.path().join("aurora-session.marker");
        assert!(write_marker(&path, &encode_marker(&[key(5), key(6)])));
        assert_eq!(read_marker(&path), Some(vec![key(5), key(6)]));
        assert_eq!(
            all_names(dir.path()),
            vec!["aurora-session.marker".to_owned()]
        );
        // The rename fails: a non-empty directory sits where it goes.
        let blocked = dir.path().join("blocked.marker");
        if let Err(err) = std::fs::create_dir_all(blocked.join("occupied")) {
            unreachable!("{err:?}");
        }
        assert!(!write_marker(&blocked, "aurora-session 1\n5\n"));
        let left = all_names(dir.path());
        assert_eq!(
            left,
            vec![
                "aurora-session.marker".to_owned(),
                "blocked.marker".to_owned()
            ]
        );
    }

    /// A symlink planted at the marker's path is replaced, never written
    /// through: only a write-then-rename can do that, so this is also
    /// what tells an atomic write from a plain `std::fs::write`.
    #[cfg(unix)]
    #[test]
    fn the_marker_write_replaces_a_planted_symlink() {
        let dir = tempdir();
        let victim = dir.path().join("victim");
        if let Err(err) = std::fs::write(&victim, b"keep") {
            unreachable!("{err:?}");
        }
        let path = dir.path().join("aurora-session.marker");
        if let Err(err) = std::os::unix::fs::symlink(&victim, &path) {
            unreachable!("{err:?}");
        }
        assert!(write_marker(&path, "aurora-session 1\n5\n"));
        assert_eq!(std::fs::read(&victim).ok(), Some(b"keep".to_vec()));
        assert_eq!(read_marker(&path), Some(vec![key(5)]));
    }

    #[test]
    fn a_marker_that_omits_a_crashed_run_still_recovers_it() {
        // 0.171.1 (L-c): a failed marker write leaves an older marker
        // that does not name a run that crashed since; the scan adds it.
        let dir = tempdir();
        for pid in [81, 82] {
            touch(&lock_path(dir.path(), pid));
            touch(&dir.path().join(session_file_name(pid, 1)));
        }
        let keys = recovery_keys(dir.path(), &[key(81)]);
        assert_eq!(
            keys,
            vec![key(81), key(82)],
            "the marker orders, the scan adds"
        );
        let claimed: Vec<RunKey> = claim_sources(dir.path(), key(83), &keys)
            .iter()
            .map(|source| source.key)
            .collect();
        assert_eq!(claimed, vec![key(81), key(82)]);
        let plan = plan_recovery(dir.path(), &claimed);
        assert!(
            plan.candidates
                .iter()
                .any(|candidate| candidate.path == dir.path().join(session_file_name(82, 1)))
        );
    }

    #[test]
    fn a_missing_lock_with_recent_files_is_left_alone_and_old_files_are_recovered() {
        // 0.171.1 (L-a): a temp cleaner removed a live run's lock file.
        let dir = tempdir();
        let file = dir.path().join(session_file_name(91, 1));
        touch(&file);
        touch(&index_path(dir.path(), 91));
        let now = std::time::SystemTime::now();
        assert!(
            claim_sources_at(dir.path(), key(92), &[key(91)], now).is_empty(),
            "recent files: possibly alive"
        );
        // Once its files are older than the window, it is a crashed run.
        let later = now + LIVENESS_WINDOW + std::time::Duration::from_mins(1);
        assert_eq!(
            claim_sources_at(dir.path(), key(92), &[key(91)], later).len(),
            1
        );
        let old = now - LIVENESS_WINDOW - std::time::Duration::from_mins(1);
        set_mtime(&file, old);
        set_mtime(&index_path(dir.path(), 91), old);
        assert_eq!(claim_sources(dir.path(), key(92), &[key(91)]).len(), 1);
    }

    #[test]
    fn a_far_future_file_time_never_blocks_recovery() {
        let dir = tempdir();
        let file = dir.path().join(session_file_name(93, 1));
        touch(&file);
        let far = std::time::SystemTime::now() + LIVENESS_WINDOW * 100;
        set_mtime(&file, far);
        assert_eq!(claim_sources(dir.path(), key(94), &[key(93)]).len(), 1);
    }

    #[test]
    fn a_deleted_lock_is_recreated_by_refresh_and_never_after_release() {
        let dir = tempdir();
        let mut ours = AutosaveNamespace::acquire(dir.path().to_path_buf(), 95);
        let lock = lock_path(dir.path(), 95);
        assert!(std::fs::remove_file(&lock).is_ok());
        let liveness = ours.liveness();
        assert!(liveness.refresh());
        assert!(lock.exists());
        assert!(try_lock(&lock, false).is_none(), "held again by this run");
        // A refresh bumps an index's time too.
        touch(&ours.index_path());
        let old = std::time::SystemTime::now() - LIVENESS_WINDOW * 4;
        set_mtime(&ours.index_path(), old);
        assert!(liveness.refresh());
        let index_time = std::fs::metadata(ours.index_path())
            .and_then(|meta| meta.modified())
            .ok();
        assert!(index_time.is_some_and(|time| time > old + LIVENESS_WINDOW));
        ours.remove_all();
        assert!(!liveness.refresh());
        assert!(!lock.exists(), "a released run never re-creates its lock");
    }

    #[test]
    fn an_older_index_write_never_overwrites_a_newer_one() {
        // 0.171.1 (L3): two writes prepared in order, landing out of
        // order; the index on disk stays at the newer listing.
        let dir = tempdir();
        let path = index_path(dir.path(), 5);
        let mut book = IndexBook::new(path.clone(), vec![entry(5, 1), entry(5, 2)], Some(1), []);
        let older: Option<IndexWrite> = book.mark_complete(1);
        let newer: Option<IndexWrite> = book.mark_complete(2);
        let (Some(older), Some(newer)) = (older, newer) else {
            unreachable!("both change the listing");
        };
        let older_temp = super::write_index_temp(&older);
        let newer_temp = super::write_index_temp(&newer);
        assert!(book.finish(newer, newer_temp));
        assert!(
            book.finish(older, older_temp),
            "the disk is at least as new"
        );
        assert_eq!(
            read_index(&path, key(5)).map(|index| index.entries),
            Ok(vec![entry(5, 1), entry(5, 2)])
        );
        assert_eq!(book.writes(), 1);
        assert_eq!(
            all_names(dir.path()),
            vec!["aurora-autosave-5.index".to_owned()]
        );
    }

    #[test]
    fn only_autosave_names_are_read_from_the_directory() {
        // 0.171.1 (L-b).
        let dir = tempdir();
        for name in [
            "unrelated.txt",
            "aurora-session.marker",
            "systemd-private-x",
            "aurora-autosave-1-1.aur",
        ] {
            touch(&dir.path().join(name));
        }
        assert_eq!(
            dir_names(dir.path()),
            vec!["aurora-autosave-1-1.aur".to_owned()]
        );
    }

    #[test]
    fn a_stale_marker_and_a_clean_quit_never_strand_a_run_that_crashes_later() {
        // 0.171.1 review L-2: B's startup marker write failed, so the old
        // marker still says [A]. A quits cleanly; then B crashes.
        let dir = tempdir();
        let marker = dir.path().join("aurora-session.marker");
        let mut a = AutosaveNamespace::acquire(dir.path().to_path_buf(), 101);
        assert!(write_marker(&marker, &encode_marker(&[a.key])));
        let b = AutosaveNamespace::acquire(dir.path().to_path_buf(), 102);
        touch(&b.session_path(1));
        touch(&b.index_path());
        // A's clean quit: its namespace goes, and the marker it leaves
        // keeps B, which the stale marker never named.
        a.remove_all();
        let remaining = after_quit(dir.path(), a.key, &read_marker(&marker).unwrap_or_default());
        assert_eq!(remaining, vec![b.key]);
        // B crashes: its lock is released, its files stay.
        drop(b);
        let found = startup_sources(dir.path(), key(103), Some(&remaining));
        assert_eq!(
            found.claimed.iter().map(|s| s.key).collect::<Vec<_>>(),
            vec![key(102)]
        );
        assert!(found.had_previous_marker);
        // Its claim holds B's lock; release it for the next start.
        drop(found);
        // And even with no marker at all, the scan still finds B, and the
        // start is a crash recovery.
        let found = startup_sources(dir.path(), key(103), None);
        assert_eq!(
            found.claimed.iter().map(|s| s.key).collect::<Vec<_>>(),
            vec![key(102)]
        );
        assert!(found.had_previous_marker);
    }

    #[test]
    fn with_no_marker_a_clean_start_is_not_a_crash() {
        let dir = tempdir();
        assert!(!startup_sources(dir.path(), key(111), None).had_previous_marker);
        // A live second Aurora with files is not a crash either.
        let live = AutosaveNamespace::acquire(dir.path().to_path_buf(), 112);
        touch(&live.session_path(1));
        let found = startup_sources(dir.path(), key(113), None);
        assert!(found.claimed.is_empty());
        assert!(!found.had_previous_marker);
        drop(live);
    }

    #[test]
    fn an_index_left_behind_by_a_failed_write_is_retried() {
        // 0.171.1 review L-1: W1 in flight, W2's write fails, W1 lands —
        // the disk lags the book until a retry brings it up to date.
        let dir = tempdir();
        let path = index_path(dir.path(), 5);
        let mut book = IndexBook::new(path.clone(), vec![entry(5, 1), entry(5, 2)], Some(1), []);
        let (Some(first), Some(second)) = (book.mark_complete(1), book.mark_complete(2)) else {
            unreachable!("both change the listing");
        };
        let first_temp = super::write_index_temp(&first);
        assert!(book.retry_write().is_none(), "writes are still in flight");
        assert!(!book.finish(second, None), "the newer write failed");
        assert!(book.finish(first, first_temp));
        assert_eq!(
            read_index(&path, key(5)).map(|index| index.entries.len()),
            Ok(1),
            "the disk lags the book"
        );
        let retry = book.retry_write();
        assert!(retry.is_some());
        assert!(apply_index_write(&mut book, retry));
        assert_eq!(read_index(&path, key(5)), Ok(book.listing()));
        assert!(book.retry_write().is_none(), "caught up");
    }

    #[test]
    fn a_gone_run_with_only_index_temps_is_retired_without_a_dialog() {
        // 0.171.1 review R-2.
        let dir = tempdir();
        // One with an unheld lock and a fresh stray temp, one without a
        // lock whose stray temp is old.
        touch(&lock_path(dir.path(), 121));
        let fresh = dir.path().join("aurora-autosave-121.index.121.0.00ff.tmp");
        touch(&fresh);
        let old = dir.path().join("aurora-autosave-122.index.122.0.00ff.tmp");
        touch(&old);
        set_mtime(&old, std::time::SystemTime::now() - LIVENESS_WINDOW * 2);
        let found = startup_sources(dir.path(), key(123), None);
        assert!(found.claimed.is_empty());
        assert!(
            !found.had_previous_marker,
            "nothing to recover, so no dialog"
        );
        drop(found);
        assert!(!fresh.exists() && !old.exists());
        assert!(!lock_path(dir.path(), 121).exists());
        // A run with a real session file beside its stray temp is a source.
        touch(&lock_path(dir.path(), 124));
        touch(&dir.path().join("aurora-autosave-124.index.124.0.00ff.tmp"));
        touch(&dir.path().join(session_file_name(124, 1)));
        assert_eq!(startup_sources(dir.path(), key(123), None).claimed.len(), 1);
    }

    #[test]
    fn a_live_runs_lock_keeps_its_files_out_of_recovery() {
        let dir = tempdir();
        let live = AutosaveNamespace::acquire(dir.path().to_path_buf(), 77);
        assert!(live.locked());
        // A crashed run leaves its lock file, but no process holds it.
        touch(&lock_path(dir.path(), 66));
        touch(&dir.path().join(session_file_name(66, 1)));
        // One that never made a lock, its files long untouched. (A gone
        // run with nothing to recover is retired, not a source: review R-2.)
        let file = dir.path().join(session_file_name(55, 1));
        touch(&file);
        set_mtime(&file, std::time::SystemTime::now() - LIVENESS_WINDOW * 2);
        let claimed = claim_sources(dir.path(), key(99), &[key(77), key(66), key(55), key(99)]);
        let sources: Vec<RunKey> = claimed.iter().map(|source| source.key).collect();
        // 77 is alive; 66 is unlocked; 55 never made a lock; 99 is us.
        assert_eq!(sources, vec![key(66), key(55)]);
        // While this run holds 66's lock, a third run cannot claim it.
        assert!(claim_sources(dir.path(), key(98), &[key(66)]).is_empty());
        drop(claimed);
        assert_eq!(claim_sources(dir.path(), key(98), &[key(66)]).len(), 1);
        drop(live);
        assert!(try_lock(&lock_path(dir.path(), 77), false).is_some());
    }

    #[test]
    fn a_reused_pid_gets_a_namespace_of_its_own() {
        // 0.171.0 review M1: a crashed run with this pid left files, so
        // this run takes the next generation and never touches them.
        let dir = tempdir();
        touch(&lock_path(dir.path(), 21));
        touch(&dir.path().join(session_file_name(21, 1)));
        let mut ours = AutosaveNamespace::acquire(dir.path().to_path_buf(), 21);
        let reused = RunKey {
            pid: 21,
            generation: 1,
        };
        assert_eq!(ours.key, reused);
        assert!(ours.locked());
        assert_eq!(
            ours.session_path(1),
            dir.path().join("aurora-autosave-21.1-1.aur")
        );
        touch(&ours.session_path(1));
        ours.remove_all();
        assert!(dir.path().join(session_file_name(21, 1)).exists());
        assert!(lock_path(dir.path(), 21).exists());
        assert!(!ours.session_path(1).exists());
    }

    #[test]
    fn a_clean_quit_removes_only_this_runs_namespace() {
        let dir = tempdir();
        let mut ours = AutosaveNamespace::acquire(dir.path().to_path_buf(), 12);
        let others = [
            "aurora-autosave-123-1.aur",
            "aurora-autosave-1-1.aur",
            "aurora-autosave-12.1-1.aur",
            "aurora-autosave-123.index",
            "aurora-autosave.aur",
        ];
        for name in others {
            touch(&dir.path().join(name));
        }
        for name in [
            "aurora-autosave-12-1.aur",
            "aurora-autosave-12-1.partial.aur",
            "aurora-autosave-12-2.aur.12.0.00ff.tmp",
        ] {
            touch(&dir.path().join(name));
        }
        touch(&ours.index_path());
        ours.remove_all();
        let mut left = super::dir_names(dir.path());
        left.sort();
        let mut expected: Vec<String> = others.iter().map(|name| (*name).to_owned()).collect();
        expected.sort();
        assert_eq!(left, expected);
        ours.remove_all();
    }

    #[test]
    fn the_marker_after_a_quit_keeps_only_runs_that_still_have_files() {
        let dir = tempdir();
        touch(&dir.path().join(session_file_name(30, 2)));
        touch(&lock_path(dir.path(), 31));
        let own = key(32);
        assert_eq!(
            marker_after_quit(dir.path(), own, &[own, key(30), key(31), key(33)]),
            vec![key(30), key(31)]
        );
        // A run the marker does not name but that has files is kept too
        // (0.171.1 review L-2); with nothing left anywhere, nothing is.
        assert_eq!(
            marker_after_quit(dir.path(), own, &[own, key(33)]),
            vec![key(30)]
        );
        let empty = tempdir();
        assert!(marker_after_quit(empty.path(), own, &[own, key(33)]).is_empty());
    }

    #[test]
    fn the_book_lists_only_complete_sessions_and_writes_only_on_change() {
        let dir = tempdir();
        let path = index_path(dir.path(), 5);
        let mut book = IndexBook::new(path.clone(), vec![entry(5, 1)], Some(1), []);
        assert!(sync(&mut book));
        assert_eq!(read_index(&path, key(5)), Err(IndexError::Missing));
        assert!(mark(&mut book, 1));
        assert_eq!(book.writes(), 1);
        // A second landing of the same session changes nothing.
        assert!(mark(&mut book, 1));
        assert_eq!(book.writes(), 1);
        // A new session is not listed until its file lands.
        assert!(set(&mut book, vec![entry(5, 1), entry(5, 2)], Some(2)));
        assert_eq!(book.writes(), 1);
        assert_eq!(
            read_index(&path, key(5)),
            Ok(AutosaveIndex {
                active: Some(1),
                entries: vec![entry(5, 1)],
            })
        );
        assert!(mark(&mut book, 2));
        assert_eq!(
            read_index(&path, key(5)),
            Ok(AutosaveIndex {
                active: Some(2),
                entries: vec![entry(5, 1), entry(5, 2)],
            })
        );
        // Closing session 1 drops it from the index, then its file.
        let one = dir.path().join(session_file_name(5, 1));
        touch(&one);
        assert!(set(&mut book, vec![entry(5, 2)], Some(2)));
        assert!(!one.exists());
        assert_eq!(
            read_index(&path, key(5)).map(|index| index.entries),
            Ok(vec![entry(5, 2)])
        );
    }
}
