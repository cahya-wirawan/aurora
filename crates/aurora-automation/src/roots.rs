//! File access for the MCP server: read roots and write roots.
//!
//! **No filesystem authority exists unless the person who starts the server
//! grants it** (`--read-root DIR`, `--write-root DIR`, both repeatable).
//! Every path a tool is given is resolved against those directories and
//! nothing else:
//!
//! * a relative path is taken from the *first* root of its kind; an
//!   absolute path must lie lexically inside one of the roots;
//! * `..` is refused outright, in any position, before the file system is
//!   asked anything;
//! * the file system is then asked through a [`cap_std::fs::Dir`] handle
//!   of the root, which resolves the path *inside* that directory and
//!   refuses a symbolic link (or `..` produced by one) that would lead
//!   out of it. There is no check-then-open gap on the way out of the
//!   root: the confinement is the open itself;
//! * a write creates its file with `create_new` (`O_EXCL`), so an existing
//!   file, a symbolic link to one and a dangling link are all refused
//!   atomically, and a write never opens an existing file for writing.
//!
//! The one place that grants ambient authority is [`Roots::open`], which
//! opens the directories the user named.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};

use crate::MAX_PATH_BYTES;

/// Why a path was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error(
        "no read root is configured, so this server may not read any file (start it with \
         --read-root DIR)"
    )]
    NoReadRoot,
    #[error(
        "no write root is configured, so this server may not write any file (start it with \
         --write-root DIR)"
    )]
    NoWriteRoot,
    #[error("the path is empty")]
    Empty,
    #[error("the path is longer than {MAX_PATH_BYTES} bytes")]
    TooLong,
    #[error("the path contains a control character")]
    ControlCharacter,
    #[error("`..` is not allowed in a path")]
    ParentComponent,
    #[error("the path is not inside any granted {0} root")]
    Outside(&'static str),
    #[error("the path leaves its root (through a symbolic link)")]
    Escapes,
    #[error("no such file or folder inside the {0} root")]
    NotFound(&'static str),
    #[error("not a regular file")]
    NotAFile,
    #[error("the file is {size} bytes; at most {max} bytes may be opened")]
    TooLarge { size: u64, max: u64 },
    #[error("`{0}` already exists; this server never overwrites a file")]
    Exists(String),
    #[error("the path has no file name")]
    NoFileName,
    #[error("the folder for `{0}` does not exist inside the write root")]
    NoFolder(String),
    #[error("file system error: {0}")]
    Io(String),
}

fn map_io(error: &io::Error, kind: &'static str) -> PathError {
    // cap-std reports a path that leaves its directory as PermissionDenied
    // ("a path led outside of the filesystem"); the kernel's own
    // RESOLVE_BENEATH refusals arrive as EXDEV, ELOOP or EPERM-ish codes.
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(18 | 40)) {
        return PathError::Escapes;
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied => PathError::Escapes,
        io::ErrorKind::NotFound => PathError::NotFound(kind),
        io::ErrorKind::AlreadyExists => PathError::Io("already exists".to_owned()),
        _ => PathError::Io(error.to_string()),
    }
}

/// One granted directory.
#[derive(Debug)]
struct Root {
    dir: Dir,
    /// The directory as the user named it, made absolute.
    named: PathBuf,
    /// Its canonical path, for absolute inputs.
    canonical: PathBuf,
}

impl Root {
    fn open(path: &Path) -> io::Result<Self> {
        let named = std::path::absolute(path)?;
        let canonical = std::fs::canonicalize(&named)?;
        let dir = Dir::open_ambient_dir(&canonical, ambient_authority())?;
        if !dir.dir_metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a root must be a directory",
            ));
        }
        Ok(Self {
            dir,
            named,
            canonical,
        })
    }

    /// The path below this root, if `absolute` lies lexically inside it.
    fn strip<'a>(&self, absolute: &'a Path) -> Option<&'a Path> {
        absolute
            .strip_prefix(&self.canonical)
            .or_else(|_| absolute.strip_prefix(&self.named))
            .ok()
    }
}

/// The directories this server may read from and write to.
#[derive(Debug, Default)]
pub struct Roots {
    read: Vec<Root>,
    write: Vec<Root>,
}

/// A file opened for reading inside a read root.
#[derive(Debug)]
pub struct ReadFile {
    pub file: std::fs::File,
    pub len: u64,
    /// The file's name (last component), for display and extension checks.
    pub name: String,
}

/// A file created inside a write root, still to be filled. Dropping it
/// without [`WriteTarget::commit`] removes the file it created.
#[derive(Debug)]
pub struct WriteTarget {
    parent: Dir,
    name: OsString,
    file: Option<std::fs::File>,
    display: String,
    committed: bool,
    /// (device, inode) of the file this value created, where the platform
    /// has them: the drop removes the name only while it still is that file.
    identity: Option<(u64, u64)>,
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // the non-Unix twin has no identity
fn identity_of(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    Some((meta.dev(), meta.ino()))
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // the non-Unix twin has no identity
fn cap_identity_of(meta: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    use cap_std::fs::MetadataExt as _;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn cap_identity_of(_: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[cfg(not(unix))]
fn identity_of(_: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

impl WriteTarget {
    /// The path as the caller named it.
    #[must_use]
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The open file.
    pub fn file(&mut self) -> Option<&mut std::fs::File> {
        self.file.as_mut()
    }

    /// Keeps the file: the write completed.
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for WriteTarget {
    fn drop(&mut self) {
        if !self.committed {
            // Close first (Windows), then remove only the name this value
            // created with `create_new`.
            self.file = None;
            // Where the platform gives file identity, only remove the name
            // if it still is the file this value created; a file someone
            // else put there since is never deleted. (Elsewhere the name is
            // removed unconditionally: a documented gap.)
            let still_ours = match (self.identity, self.parent.symlink_metadata(&self.name)) {
                (Some(ours), Ok(now)) => cap_identity_of(&now) == Some(ours),
                (None, Ok(_)) => true,
                (_, Err(_)) => false,
            };
            if still_ours && let Err(error) = self.parent.remove_file(&self.name) {
                tracing::warn!(%error, file = %self.display, "couldn't remove a partial file");
            }
        }
    }
}

/// Checks the text of a path before any file system access.
fn check_text(user: &str) -> Result<(), PathError> {
    if user.is_empty() {
        return Err(PathError::Empty);
    }
    if user.len() > MAX_PATH_BYTES {
        return Err(PathError::TooLong);
    }
    if user.chars().any(char::is_control) {
        return Err(PathError::ControlCharacter);
    }
    Ok(())
}

/// A path's components as a relative path made only of names; `..` and
/// anything that would restart at a root is refused.
fn names_only(path: &Path) -> Result<PathBuf, PathError> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir => return Err(PathError::ParentComponent),
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathError::Outside("granted"));
            }
        }
    }
    Ok(out)
}

impl Roots {
    /// No roots: no file can be read or written.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Opens the named directories. This is the only place the server
    /// takes ambient file system authority, and it takes it for exactly
    /// these directories.
    ///
    /// # Errors
    /// A directory that does not exist or is not a directory.
    pub fn open(read: &[PathBuf], write: &[PathBuf]) -> io::Result<Self> {
        let open_all = |paths: &[PathBuf]| -> io::Result<Vec<Root>> {
            paths
                .iter()
                .map(|p| {
                    Root::open(p)
                        .map_err(|e| io::Error::new(e.kind(), format!("root {}: {e}", p.display())))
                })
                .collect()
        };
        Ok(Self {
            read: open_all(read)?,
            write: open_all(write)?,
        })
    }

    /// Whether any read root is granted.
    #[must_use]
    pub fn can_read(&self) -> bool {
        !self.read.is_empty()
    }

    /// Whether any write root is granted.
    #[must_use]
    pub fn can_write(&self) -> bool {
        !self.write.is_empty()
    }

    fn pick<'a>(
        roots: &'a [Root],
        user: &str,
        kind: &'static str,
    ) -> Result<(&'a Root, PathBuf), PathError> {
        check_text(user)?;
        let path = Path::new(user);
        // `..` is refused on the text, before and independently of any
        // file system call.
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(PathError::ParentComponent);
        }
        if path.is_absolute() {
            for root in roots {
                if let Some(rest) = root.strip(path) {
                    return Ok((root, names_only(rest)?));
                }
            }
            return Err(PathError::Outside(kind));
        }
        let root = roots.first().ok_or(PathError::Outside(kind))?;
        Ok((root, names_only(path)?))
    }

    /// Opens `user` for reading inside a read root.
    ///
    /// # Errors
    /// The path is refused ([`PathError`]), missing, not a regular file or
    /// larger than `max_bytes`.
    pub fn open_read(&self, user: &str, max_bytes: u64) -> Result<ReadFile, PathError> {
        if self.read.is_empty() {
            return Err(PathError::NoReadRoot);
        }
        let (root, rel) = Self::pick(&self.read, user, "read")?;
        let name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(PathError::NoFileName)?
            .to_owned();
        // Metadata first (following links, confined): a FIFO or a device
        // must not be opened, since opening a FIFO can block.
        let metadata = root.dir.metadata(&rel).map_err(|e| map_io(&e, "read"))?;
        if !metadata.is_file() {
            return Err(PathError::NotAFile);
        }
        if metadata.len() > max_bytes {
            return Err(PathError::TooLarge {
                size: metadata.len(),
                max: max_bytes,
            });
        }
        // Opened non-blocking: if the name was swapped for a FIFO since the
        // metadata call, the open returns at once and the handle check
        // below refuses it instead of waiting for a writer forever.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt as _;
            options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed());
        }
        let file = root
            .dir
            .open_with(&rel, &options)
            .map_err(|e| map_io(&e, "read"))?;
        let opened = file.metadata().map_err(|e| map_io(&e, "read"))?;
        if !opened.is_file() {
            return Err(PathError::NotAFile);
        }
        if opened.len() > max_bytes {
            return Err(PathError::TooLarge {
                size: opened.len(),
                max: max_bytes,
            });
        }
        Ok(ReadFile {
            file: file.into_std(),
            len: opened.len(),
            name,
        })
    }

    fn parent_and_name(root: &Root, rel: &Path, user: &str) -> Result<(Dir, OsString), PathError> {
        let name = rel.file_name().ok_or(PathError::NoFileName)?.to_owned();
        let folder = rel.parent().unwrap_or_else(|| Path::new(""));
        let parent = if folder.as_os_str().is_empty() {
            root.dir.try_clone()
        } else {
            root.dir.open_dir(folder)
        }
        .map_err(|e| match map_io(&e, "write") {
            PathError::NotFound(_) => PathError::NoFolder(user.to_owned()),
            other => other,
        })?;
        Ok((parent, name))
    }

    /// Resolves a write path and checks it does not exist yet, without
    /// creating anything: a fast refusal before an expensive export.
    /// [`Roots::create_write`] repeats the check atomically.
    ///
    /// # Errors
    /// The path is refused, its folder is missing or the target exists.
    pub fn check_write(&self, user: &str) -> Result<(), PathError> {
        if self.write.is_empty() {
            return Err(PathError::NoWriteRoot);
        }
        let (root, rel) = Self::pick(&self.write, user, "write")?;
        let (parent, name) = Self::parent_and_name(root, &rel, user)?;
        match parent.symlink_metadata(&name) {
            Ok(_) => Err(PathError::Exists(user.to_owned())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(map_io(&e, "write")),
        }
    }

    /// Creates `user` inside a write root, atomically and only if nothing
    /// is there (`O_EXCL`).
    ///
    /// # Errors
    /// The path is refused, its folder is missing or the target exists
    /// (any kind of entry, a dangling symbolic link included).
    pub fn create_write(&self, user: &str) -> Result<WriteTarget, PathError> {
        if self.write.is_empty() {
            return Err(PathError::NoWriteRoot);
        }
        let (root, rel) = Self::pick(&self.write, user, "write")?;
        let (parent, name) = Self::parent_and_name(root, &rel, user)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let file = parent.open_with(&name, &options).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                PathError::Exists(user.to_owned())
            } else {
                map_io(&e, "write")
            }
        })?;
        let file = file.into_std();
        let identity = file.metadata().ok().and_then(|m| identity_of(&m));
        Ok(WriteTarget {
            parent,
            name,
            identity,
            file: Some(file),
            display: user.to_owned(),
            committed: false,
        })
    }
}

/// Copies up to `max_bytes` of `from` into `to` in chunks, calling
/// `step(done)` after each; stops with `Ok(false)` when `step` says stop.
///
/// # Errors
/// Any read or write error; reading more than `max_bytes` is an error.
pub fn copy_bounded(
    from: &mut impl Read,
    to: &mut impl Write,
    max_bytes: u64,
    mut step: impl FnMut(u64) -> bool,
) -> io::Result<bool> {
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            return Ok(true);
        }
        done = done.saturating_add(n as u64);
        if done > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the file is larger than its limit",
            ));
        }
        if let Some(chunk) = buffer.get(..n) {
            to.write_all(chunk)?;
        }
        if !step(done) {
            return Ok(false);
        }
    }
}
