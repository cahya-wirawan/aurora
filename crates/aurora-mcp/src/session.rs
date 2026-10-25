//! What the server needs from a document host.
//!
//! The server (this crate) knows the protocol, the file-access roots and the
//! command registry; it knows no document. The host (`aurora-app`'s
//! headless session, wired in by `aurora-cli`) opens, edits, renders and
//! saves one document and answers in JSON. Everything crossing this
//! boundary is plain data, and the host never sees a path the roots did not
//! confine: it is handed private scratch files, not the caller's paths.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aurora_commands::{CommandContext, Validated};
use serde_json::Value;

/// A flag a long operation polls to learn the client cancelled.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the operation cancelled.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether a cancel was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Why a host operation did not complete.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// There is no document yet.
    #[error("no document is open; open one with doc_open first")]
    NoDocument,
    /// The command exists but cannot run in a headless session.
    #[error("{0}")]
    NotAvailable(String),
    /// The request was understood and refused (a limit, a rule).
    #[error("{0}")]
    Refused(String),
    /// The operation failed.
    #[error("{0}")]
    Failed(String),
    /// The client cancelled.
    #[error("cancelled")]
    Cancelled,
}

/// A rendered preview.
#[derive(Clone, Debug)]
pub struct Preview {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub document_width: u32,
    pub document_height: u32,
}

/// A file format `doc_save` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Jpeg,
    Tiff,
    Psd,
    Aur,
}

impl ExportFormat {
    /// The format an extension names (case-insensitive).
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "tif" | "tiff" => Some(Self::Tiff),
            "psd" => Some(Self::Psd),
            "aur" => Some(Self::Aur),
            _ => None,
        }
    }

    /// The extension the host's scratch file gets.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::Psd => "psd",
            Self::Aur => "aur",
        }
    }
}

/// What an export did.
#[derive(Clone, Debug)]
pub enum ExportOutcome {
    /// The file was written to the scratch path. `report` lists anything
    /// the format could not keep (empty for a lossless save).
    Written { report: Value },
    /// A PSD save would lose or change something and the caller did not
    /// accept that; nothing was written.
    NeedsConfirmation { report: Value },
}

/// One document host. Implemented over the application's own document
/// code; every method runs on a blocking thread, one at a time.
pub trait Session: Send + 'static {
    /// Tells the host where it may keep private scratch files (its tile
    /// store): a directory inside the server's own locked scratch
    /// directory, so it is swept with it after a crash. Called once,
    /// before any other method.
    fn set_scratch_dir(&mut self, _dir: &Path) {}

    /// Replaces the session's document with the file at `staged`, a private
    /// copy of what the caller named; `name` is the original file name. The
    /// server has already checked PNG, JPEG, TIFF, PSD and PSB headers
    /// against `max_pixels`; the host must do the same for the formats the
    /// server cannot read (`.aur`) before decoding.
    ///
    /// # Errors
    /// The file could not be opened.
    fn open(
        &mut self,
        staged: &Path,
        name: &str,
        max_pixels: u64,
        cancel: &Cancel,
    ) -> Result<Value, SessionError>;

    /// A worst-case size in bytes of an export in `format`, to refuse early
    /// when the scratch disk clearly cannot hold it; `None` when unknown.
    fn export_estimate(&mut self, _format: ExportFormat) -> Option<u64> {
        None
    }

    /// The document as JSON (layer tree, history, selection, canvas size);
    /// `{"document": null}` when none is open.
    ///
    /// # Errors
    /// The document could not be described.
    fn inspect(&mut self) -> Result<Value, SessionError>;

    /// Whether a document is open.
    fn has_document(&self) -> bool;

    /// The facts the registry's enabled checks need.
    fn command_context(&mut self) -> CommandContext;

    /// Ids of the registry commands [`Session::run_command`] can run.
    fn headless_commands(&self) -> &'static [&'static str];

    /// Runs a command whose arguments the registry already validated.
    ///
    /// # Errors
    /// [`SessionError::NotAvailable`] for a command that needs a window.
    fn run_command(&mut self, call: &Validated) -> Result<Value, SessionError>;

    /// A flattened preview no larger than `max_side` on its longest side.
    ///
    /// # Errors
    /// No document, or the document could not be composited.
    fn preview(&mut self, max_side: u32, cancel: &Cancel) -> Result<Preview, SessionError>;

    /// Writes the document to `out`, a fresh private path, in `format`.
    /// `progress` takes a fraction 0.0..=1.0 and may be called often.
    ///
    /// # Errors
    /// No document, a refusal, a failure or a cancel.
    fn export(
        &mut self,
        format: ExportFormat,
        accept_losses: bool,
        out: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(f64),
    ) -> Result<ExportOutcome, SessionError>;

    /// The grouped report a PSD save would give, without writing a file.
    ///
    /// # Errors
    /// No document, or the check could not run.
    fn psd_check(&mut self, cancel: &Cancel) -> Result<Value, SessionError>;
}
