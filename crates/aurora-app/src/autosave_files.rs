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

fn dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
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
pub(crate) fn recovery_keys(dir: &Path, previous: &[RunKey]) -> Vec<RunKey> {
    let data = keys_with_data(&dir_names(dir));
    if previous.is_empty() {
        return data.into_iter().take(MARKER_MAX_KEYS).collect();
    }
    prioritised(previous.iter().copied(), &data, MARKER_MAX_KEYS)
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
pub(crate) fn claim_sources(dir: &Path, own: RunKey, marker: &[RunKey]) -> Vec<RecoverySource> {
    let mut sources = Vec::new();
    for &key in marker.iter().filter(|&&key| key != own) {
        let path = lock_path(dir, key);
        if !path.exists() {
            sources.push(RecoverySource { key, _lock: None });
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
    sources
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
pub(crate) fn marker_after_quit(dir: &Path, own: RunKey, marker: &[RunKey]) -> Vec<RunKey> {
    inherited_marker_keys(dir, own, marker)
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

/// This run's own corner of the autosave directory: its key and the lock
/// that tells other runs it is alive.
#[derive(Debug)]
pub(crate) struct AutosaveNamespace {
    pub(crate) dir: PathBuf,
    pub(crate) key: RunKey,
    lock: Option<File>,
}

impl AutosaveNamespace {
    /// Picks this run's key — `pid`, or the first `pid.<generation>` with
    /// nothing on disk when a crashed run with the same pid left files
    /// (0.171.0 review M1) — and takes its lock (logged, not fatal, if it
    /// cannot: the run then looks crashed to a second Aurora, which is
    /// the pre-0.171.0 behaviour).
    pub(crate) fn acquire(dir: PathBuf, pid: u32) -> Self {
        let names = dir_names(&dir);
        for generation in 0..MAX_GENERATIONS {
            let key = RunKey { pid, generation };
            if has_files(key, &names) {
                continue;
            }
            let lock = try_lock(&lock_path(&dir, key), true);
            if lock.is_none() {
                tracing::warn!(%key, "could not take this run's autosave lock");
            }
            return Self { dir, key, lock };
        }
        tracing::warn!(pid, "every autosave generation of this pid is taken");
        let key = RunKey {
            pid,
            generation: MAX_GENERATIONS,
        };
        Self {
            dir,
            key,
            lock: None,
        }
    }

    /// Whether this run holds its lock.
    #[cfg(test)]
    pub(crate) fn locked(&self) -> bool {
        self.lock.is_some()
    }

    /// Where session `id`'s autosave lives.
    pub(crate) fn session_path(&self, id: u64) -> PathBuf {
        self.dir.join(session_file_name(self.key, id))
    }

    /// This run's index.
    pub(crate) fn index_path(&self) -> PathBuf {
        index_path(&self.dir, self.key)
    }

    /// A clean quit's cleanup: every file in this run's namespace and its
    /// index, then the lock is released and its file removed. Only this
    /// run's key is touched — never a crashed run's, even one with the
    /// same pid (it has another generation). Idempotent.
    pub(crate) fn remove_all(&mut self) {
        remove_session_files(&self.dir, self.key);
        remove_quietly(&self.index_path());
        drop(self.lock.take());
        remove_quietly(&lock_path(&self.dir, self.key));
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
    written: Option<AutosaveIndex>,
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
            written: None,
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

    /// Writes the index if its listing changed; `true` when the file on
    /// disk now matches (or there is still nothing to list).
    pub(crate) fn sync(&mut self) -> bool {
        let listing = self.listing();
        if self.written.as_ref() == Some(&listing)
            || (self.written.is_none() && listing.entries.is_empty())
        {
            return true;
        }
        if write_index(&self.path, &listing) {
            self.written = Some(listing);
            self.writes += 1;
            true
        } else {
            false
        }
    }

    /// Session `id`'s file landed complete; rewrites the index if that
    /// adds it.
    pub(crate) fn mark_complete(&mut self, id: u64) -> bool {
        self.complete.insert(id);
        self.sync()
    }

    /// Replaces the session set (creation, closing) and the active one.
    /// Once the index without them has landed, removed sessions' files
    /// are deleted — never before, so a crash in between still has them.
    pub(crate) fn set_sessions(&mut self, order: Vec<IndexEntry>, active: Option<u64>) -> bool {
        let removed: Vec<IndexEntry> = self
            .order
            .iter()
            .filter(|old| !order.iter().any(|new| new.id == old.id))
            .cloned()
            .collect();
        self.order = order;
        self.active = active;
        let dir = self.path.parent().map(Path::to_path_buf);
        for entry in &removed {
            self.complete.remove(&entry.id);
        }
        if !self.sync() {
            return false;
        }
        if let Some(dir) = dir {
            for entry in removed {
                let file = dir.join(&entry.file);
                remove_quietly(&file);
                remove_quietly(&crate::partial_autosave_path(&file));
            }
        }
        true
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

    use super::{inherited_marker_keys, recovery_keys, retire_namespace, write_marker};

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
            super::dir_names(dir.path()),
            vec!["aurora-session.marker".to_owned()]
        );
        // The rename fails: a non-empty directory sits where it goes.
        let blocked = dir.path().join("blocked.marker");
        if let Err(err) = std::fs::create_dir_all(blocked.join("occupied")) {
            unreachable!("{err:?}");
        }
        assert!(!write_marker(&blocked, "aurora-session 1\n5\n"));
        let mut left = super::dir_names(dir.path());
        left.sort();
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
    fn a_live_runs_lock_keeps_its_files_out_of_recovery() {
        let dir = tempdir();
        let live = AutosaveNamespace::acquire(dir.path().to_path_buf(), 77);
        assert!(live.locked());
        // A crashed run leaves its lock file, but no process holds it.
        touch(&lock_path(dir.path(), 66));
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
        assert!(marker_after_quit(dir.path(), own, &[own, key(33)]).is_empty());
    }

    #[test]
    fn the_book_lists_only_complete_sessions_and_writes_only_on_change() {
        let dir = tempdir();
        let path = index_path(dir.path(), 5);
        let mut book = IndexBook::new(path.clone(), vec![entry(5, 1)], Some(1), []);
        assert!(book.sync());
        assert_eq!(read_index(&path, key(5)), Err(IndexError::Missing));
        assert!(book.mark_complete(1));
        assert_eq!(book.writes(), 1);
        // A second landing of the same session changes nothing.
        assert!(book.mark_complete(1));
        assert_eq!(book.writes(), 1);
        // A new session is not listed until its file lands.
        assert!(book.set_sessions(vec![entry(5, 1), entry(5, 2)], Some(2)));
        assert_eq!(book.writes(), 1);
        assert_eq!(
            read_index(&path, key(5)),
            Ok(AutosaveIndex {
                active: Some(1),
                entries: vec![entry(5, 1)],
            })
        );
        assert!(book.mark_complete(2));
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
        assert!(book.set_sessions(vec![entry(5, 2)], Some(2)));
        assert!(!one.exists());
        assert_eq!(
            read_index(&path, key(5)).map(|index| index.entries),
            Ok(vec![entry(5, 2)])
        );
    }
}
