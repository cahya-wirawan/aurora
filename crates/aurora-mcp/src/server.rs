//! The MCP tools, resources and their security rules.
//!
//! Tool names are `snake_case` (dots are not valid in every client's
//! grammar). Every tool carries all four annotation hints; they are honest
//! hints, not permissions. Arguments are strict: an unknown tool argument,
//! a wrong type, an unknown command id or command argument is a JSON-RPC
//! `-32602` error returned **before anything runs**. A command that was
//! understood but cannot run (disabled, needs a window, a file problem) is
//! a tool result with `isError: true`.

use std::io::{Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use aurora_commands::{ArgValue, Args, Registry, Validated};
use base64::Engine as _;
use rmcp::RoleServer;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock as Content,
    Implementation, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::budgets::{Limits, MAX_PREVIEW_PNG_BYTES, MAX_RESPONSE_BYTES};
use crate::header;
use crate::roots::{PathError, Roots, copy_bounded};
use crate::scratch::{Scratch, free_bytes};
use crate::session::{Cancel, ExportFormat, ExportOutcome, Session, SessionError};

/// Longest accepted progress token: it is echoed in every progress notification.
const MAX_PROGRESS_TOKEN_BYTES: usize = 256;

const DOCUMENT: &str = "aurora://document";
const COMMANDS: &str = "aurora://commands";

const INSTRUCTIONS: &str = "Aurora image editor, headless. Discover commands with command_list \
(each says whether it can run in this headless server), open a file with doc_open, inspect with \
doc_inspect, edit with command_run or command_batch, look with render_preview, check a PSD save \
with psd_check and write with doc_save (it never overwrites an existing file). File paths are \
relative to the roots granted at launch (--read-root, --write-root); without a root there is no \
file access. Read aurora://document and aurora://commands for live JSON.";

/// Commands the server itself runs (they touch files, which only the server
/// may do, through its roots).
const SERVER_COMMANDS: &[&str] = &["file.open", "file.save"];

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// Only commands whose id or title contains this text (case-insensitive).
    #[serde(default)]
    pub filter: Option<String>,
    /// Only commands that can run right now.
    #[serde(default)]
    pub enabled_only: Option<bool>,
    /// Only commands this headless server can run.
    #[serde(default)]
    pub headless_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunParams {
    /// Command id, e.g. `layer.new`, `select.expand`, from `command_list`.
    pub id: String,
    /// The command's arguments by name (booleans, numbers or text). Unknown
    /// names are refused before the command runs.
    #[serde(default)]
    pub args: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchParams {
    /// Commands to run in order. All of them are checked (ids, argument
    /// names and types) before the first runs.
    pub steps: Vec<RunParams>,
    /// Stop at the first failing step (default true).
    #[serde(default)]
    pub stop_on_error: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// File to open (PNG, JPEG, TIFF, PSD, PSB or .aur), relative to the read
    /// root or an absolute path inside it.
    pub path: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    /// Longest side in pixels (default 1024, at most 2048).
    #[serde(default)]
    pub max_side: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveParams {
    /// Target file inside the write root; the extension picks the format
    /// (png, jpg, tif, psd, aur). The file must not exist.
    pub path: String,
    /// For a PSD that would lose or change something: write it anyway. By
    /// default such a save is refused and the report is returned instead.
    #[serde(default)]
    pub accept_losses: Option<bool>,
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// Why a tool did not produce a result: a tool error (`isError: true`).
/// (A bad argument never gets this far: it is a JSON-RPC `-32602` error
/// returned before the tool runs.)
#[derive(Debug)]
pub(crate) enum Fail {
    Tool(String),
}

impl From<PathError> for Fail {
    fn from(error: PathError) -> Self {
        Self::Tool(error.to_string())
    }
}

impl From<SessionError> for Fail {
    fn from(error: SessionError) -> Self {
        Self::Tool(error.to_string())
    }
}

fn fail_result(message: impl Into<String>) -> CallToolResult {
    let mut text: String = message.into();
    if text.len() > 16 * 1024 {
        text = text.chars().take(8 * 1024).collect::<String>() + "... (message cut)";
    }
    CallToolResult::error(vec![Content::text(text)])
}

fn json_text(value: &Value) -> Result<String, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(format!(
            "the reply is {} bytes; at most {MAX_RESPONSE_BYTES} are returned; the operation may \
             have completed",
            text.len()
        ));
    }
    Ok(text)
}

fn ok_json(value: &Value) -> CallToolResult {
    match json_text(value) {
        Ok(text) => CallToolResult::success(vec![Content::text(text)]),
        Err(message) => fail_result(message),
    }
}

// The tool functions return `Result<_, McpError>`; this keeps their bodies uniform.
#[allow(clippy::unnecessary_wraps)]
fn to_result(outcome: Result<Value, Fail>) -> Result<CallToolResult, McpError> {
    match outcome {
        Ok(value) => Ok(ok_json(&value)),
        Err(Fail::Tool(message)) => Ok(fail_result(message)),
    }
}

/// A JSON argument as a registry argument.
fn arg_value(name: &str, value: &Value) -> Result<ArgValue, String> {
    match value {
        Value::Bool(b) => Ok(ArgValue::Bool(*b)),
        Value::String(s) => Ok(ArgValue::Text(s.clone())),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(ArgValue::Int(i))
            } else {
                n.as_f64()
                    .map(ArgValue::Number)
                    .ok_or_else(|| format!("`{name}` is not a usable number"))
            }
        }
        _ => Err(format!("`{name}` must be a boolean, a number or text")),
    }
}

fn to_args(raw: Option<&Map<String, Value>>) -> Result<Args, String> {
    let mut args = Args::new();
    if let Some(map) = raw {
        for (name, value) in map {
            args.insert(name.clone(), arg_value(name, value)?);
        }
    }
    Ok(args)
}

/// Checks a command call against the registry: the id, the argument names,
/// types and ranges. Nothing runs.
fn validate_step(step: &RunParams) -> Result<Validated, String> {
    let args = to_args(step.args.as_ref())?;
    Registry::builtin()
        .validate_args(&step.id, &args)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

struct Inner {
    /// The document host. **Never locked on an async worker**: only inside
    /// `spawn_blocking` closures (`AuroraMcp::spawn`), so a long export or
    /// preview holding it cannot stop the runtime from serving ping,
    /// cancellation or other requests.
    session: Mutex<Box<dyn Session>>,
    /// The host's static command list, read once at start so no request has
    /// to take the lock to answer "can this run headless?".
    headless: &'static [&'static str],
    roots: Roots,
    limits: Limits,
    scratch: Scratch,
    counter: AtomicU64,
}

/// The server: one document session, the granted roots and the limits.
#[derive(Clone)]
pub struct AuroraMcp {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for AuroraMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuroraMcp")
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

/// A scratch file that is removed when dropped.
struct ScratchFile(PathBuf);

impl Drop for ScratchFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "couldn't remove a scratch file");
        }
    }
}

/// A scratch directory (one export's) that is removed when dropped.
struct ScratchDir(PathBuf);

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "couldn't remove a scratch directory");
        }
    }
}

/// Stops a watcher task when dropped.
struct Watch(Option<tokio::task::JoinHandle<()>>);

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Creates a private (`0700` on Unix) directory.
fn private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Creates a new private file (`0600` on Unix), never opening an existing one.
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// An extension that is safe to put in a scratch file name.
fn safe_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.chars()
                .filter(char::is_ascii_alphanumeric)
                .take(8)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "bin".to_owned())
}

/// The total size of the files directly in `dir`.
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len())
            .sum()
    })
}

/// One step's entry in a batch reply.
fn step_entry(index: usize, id: &str, outcome: Result<Value, Fail>) -> (Value, bool) {
    match outcome {
        Ok(result) => (json!({"step": index, "id": id, "ok": true, "result": result}), true),
        Err(Fail::Tool(message)) => (
            json!({"step": index, "id": id, "ok": false, "error": message}),
            false,
        ),
    }
}

/// Check-and-run of one host command, **in one lock hold**: the enabled
/// check and the run see the same document.
fn run_host_command(session: &mut dyn Session, call: &Validated) -> Result<Value, Fail> {
    if !session.has_document() {
        return Err(SessionError::NoDocument.into());
    }
    if let Err(reason) = Registry::builtin().check_enabled(&call.id, &session.command_context()) {
        return Err(Fail::Tool(format!("`{}` is not enabled: {reason}", call.id)));
    }
    Ok(session.run_command(call)?)
}

impl AuroraMcp {
    /// Builds a server over `session` with the granted `roots`.
    ///
    /// # Errors
    /// The private scratch directory could not be created.
    pub fn new(
        mut session: Box<dyn Session>,
        roots: Roots,
        limits: Limits,
        scratch_parent: Option<&Path>,
    ) -> std::io::Result<Self> {
        let scratch = Scratch::create(scratch_parent)?;
        let host_dir = scratch.path().join("host");
        private_dir(&host_dir)?;
        session.set_scratch_dir(&host_dir);
        let headless = session.headless_commands();
        let mut tool_router = Self::tool_router();
        for route in tool_router.map.values_mut() {
            let mut schema = (*route.attr.input_schema).clone();
            schema.insert("additionalProperties".into(), json!(false));
            route.attr.input_schema = Arc::new(schema);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                session: Mutex::new(session),
                headless,
                roots,
                limits,
                scratch,
                counter: AtomicU64::new(1),
            }),
            tool_router,
        })
    }

    /// The server's private scratch directory (for tests and diagnostics).
    #[must_use]
    pub fn scratch_dir(&self) -> &Path {
        self.inner.scratch.path()
    }

    fn next_counter(&self) -> u64 {
        self.inner.counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Runs `job` on a blocking thread. `ctx`'s cancellation is forwarded to
    /// `cancel`, which `job` polls.
    async fn spawn<T: Send + 'static>(
        ctx: Option<&RequestContext<RoleServer>>,
        cancel: Cancel,
        job: impl FnOnce(&Cancel) -> Result<T, SessionError> + Send + 'static,
    ) -> Result<T, SessionError> {
        let watch = Watch(ctx.map(|ctx| {
            let token = ctx.ct.clone();
            let flag = cancel.clone();
            tokio::spawn(async move {
                token.cancelled().await;
                flag.cancel();
            })
        }));
        let joined = tokio::task::spawn_blocking(move || job(&cancel)).await;
        drop(watch);
        joined.unwrap_or_else(|e| Err(SessionError::Failed(format!("the operation failed: {e}"))))
    }

    /// Runs `f` on the session, on a blocking thread, under its lock.
    async fn blocking<T: Send + 'static>(
        &self,
        ctx: Option<&RequestContext<RoleServer>>,
        f: impl FnOnce(&mut dyn Session, &Cancel) -> Result<T, SessionError> + Send + 'static,
    ) -> Result<T, SessionError> {
        self.blocking_with(ctx, Cancel::new(), f).await
    }

    async fn blocking_with<T: Send + 'static>(
        &self,
        ctx: Option<&RequestContext<RoleServer>>,
        cancel: Cancel,
        f: impl FnOnce(&mut dyn Session, &Cancel) -> Result<T, SessionError> + Send + 'static,
    ) -> Result<T, SessionError> {
        let inner = Arc::clone(&self.inner);
        Self::spawn(ctx, cancel, move |cancel| {
            // A panic poisons the lock; the session is used again rather
            // than failing every later call (a release build aborts on a
            // panic anyway, so this is only reachable in dev builds).
            let mut guard = inner.session.lock().unwrap_or_else(PoisonError::into_inner);
            f(guard.as_mut(), cancel)
        })
        .await
    }

    // ----- shared implementations -----

    /// Reads the confined file once into the scratch directory, after the
    /// header size check. Runs without the session lock.
    async fn stage(
        &self,
        path: String,
        ctx: Option<&RequestContext<RoleServer>>,
    ) -> Result<(ScratchFile, String), SessionError> {
        let inner = Arc::clone(&self.inner);
        let counter = self.next_counter();
        Self::spawn(ctx, Cancel::new(), move |cancel| {
            let limits = inner.limits;
            let mut read = inner
                .roots
                .open_read(&path, limits.max_open_bytes)
                .map_err(|e| SessionError::Refused(e.to_string()))?;
            // The header says how big the picture is before a byte is
            // decoded or copied.
            match header::dimensions(&mut read.file) {
                Err(message) => return Err(SessionError::Refused(message)),
                Ok(Some(d)) if d.width == 0 || d.height == 0 => {
                    return Err(SessionError::Refused(format!(
                        "the {} header declares an empty picture",
                        d.format
                    )));
                }
                Ok(Some(d)) if d.pixels() > limits.max_open_pixels => {
                    return Err(SessionError::Refused(format!(
                        "the {} header declares {} x {} px ({} Mpx); at most {} Mpx may be \
                         opened (raise it with --max-open-mpx)",
                        d.format,
                        d.width,
                        d.height,
                        d.pixels() / 1_000_000,
                        limits.max_open_pixels / 1_000_000
                    )));
                }
                Ok(_) => {}
            }
            read.file
                .seek(std::io::SeekFrom::Start(0))
                .map_err(|e| SessionError::Failed(format!("reading {}: {e}", read.name)))?;
            let extension = safe_extension(&read.name);
            let staged = ScratchFile(
                inner
                    .scratch
                    .path()
                    .join(format!("open-{counter}.{extension}")),
            );
            let mut out = private_file(&staged.0)
                .map_err(|e| SessionError::Failed(format!("scratch file: {e}")))?;
            let completed = copy_bounded(&mut read.file, &mut out, limits.max_open_bytes, |_| {
                !cancel.is_cancelled()
            })
            .map_err(|e| SessionError::Failed(format!("reading {}: {e}", read.name)))?;
            if !completed {
                return Err(SessionError::Cancelled);
            }
            out.flush()
                .map_err(|e| SessionError::Failed(format!("scratch file: {e}")))?;
            Ok((staged, read.name))
        })
        .await
    }

    async fn open_impl(
        &self,
        path: String,
        ctx: Option<&RequestContext<RoleServer>>,
    ) -> Result<Value, Fail> {
        let (staged, name) = self.stage(path, ctx).await?;
        let max_pixels = self.inner.limits.max_open_pixels;
        // The host reads the private copy, never the caller's path.
        Ok(self
            .blocking(ctx, move |session, cancel| {
                session.open(&staged.0, &name, max_pixels, cancel)
            })
            .await?)
    }

    #[allow(clippy::too_many_lines)]
    async fn save_impl(
        &self,
        params: SaveParams,
        ctx: Option<&RequestContext<RoleServer>>,
    ) -> Result<Value, Fail> {
        let extension = Path::new(&params.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let Some(format) = ExportFormat::from_extension(extension) else {
            return Err(Fail::Tool(format!(
                "`{}` has no supported extension; use .png, .jpg, .tif, .psd or .aur",
                params.path
            )));
        };
        let path = params.path;
        // Refuse a bad or existing target before queueing behind the
        // session; the closure below checks again under its lock.
        {
            let inner = Arc::clone(&self.inner);
            let path = path.clone();
            Self::spawn(ctx, Cancel::new(), move |_| {
                inner
                    .roots
                    .check_write(&path)
                    .map_err(|e| SessionError::Refused(e.to_string()))
            })
            .await?;
        }
        let limits = self.inner.limits;
        // One private directory per export; the size monitor watches it.
        let dir = ScratchDir(self.inner.scratch.path().join(format!("export-{}", self.next_counter())));
        private_dir(&dir.0).map_err(|e| Fail::Tool(format!("scratch directory: {e}")))?;
        let out = dir.0.join(format!("out.{}", format.extension()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
        let this = self.clone();
        let accept = params.accept_losses.unwrap_or(false);
        let cancel = Cancel::new();
        // Bound the file *while it is written*: a monitor cancels the
        // export as soon as its directory outgrows the limit.
        let over = Arc::new(AtomicBool::new(false));
        let _monitor = Watch(Some({
            let (dir, cancel, over) = (dir.0.clone(), cancel.clone(), Arc::clone(&over));
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    let size = tokio::task::spawn_blocking({
                        let dir = dir.clone();
                        move || dir_bytes(&dir)
                    })
                    .await
                    .unwrap_or(0);
                    if size > limits.max_export_bytes {
                        over.store(true, Ordering::SeqCst);
                        cancel.cancel();
                        break;
                    }
                }
            })
        }));
        let job = self.blocking_with(ctx, cancel, move |session, cancel| {
            // Re-checked under the session lock: a concurrent save to the
            // same name that finished while this one queued must not make
            // this one run a whole export for nothing.
            this.inner
                .roots
                .check_write(&path)
                .map_err(|e| SessionError::Refused(e.to_string()))?;
            if !session.has_document() {
                return Err(SessionError::NoDocument);
            }
            if let (Some(estimate), Some(free)) = (
                session.export_estimate(format),
                free_bytes(this.inner.scratch.path()),
            ) {
                let need = estimate.min(limits.max_export_bytes);
                if free < need {
                    return Err(SessionError::Refused(format!(
                        "the scratch disk has {} MiB free; this export may need {} MiB (use \
                         --scratch-dir to put it elsewhere)",
                        free >> 20,
                        need >> 20
                    )));
                }
            }
            let outcome = session.export(format, accept, &out, cancel, &mut |f| {
                let _ = tx.send(f.clamp(0.0, 1.0) * 0.9);
            })?;
            let report = match outcome {
                ExportOutcome::NeedsConfirmation { report } => {
                    return Ok(json!({
                        "saved": false,
                        "needs_confirmation": true,
                        "message": "This save would lose or change something. Nothing was written. Review the report and call doc_save again with accept_losses: true to write it anyway.",
                        "report": report,
                    }));
                }
                ExportOutcome::Written { report } => report,
            };
            if cancel.is_cancelled() {
                return Err(SessionError::Cancelled);
            }
            let mut source = std::fs::File::open(&out)
                .map_err(|e| SessionError::Failed(format!("reading the export: {e}")))?;
            let total = source.metadata().map_or(0, |m| m.len());
            if total > limits.max_export_bytes {
                return Err(SessionError::Refused(export_too_large(limits)));
            }
            let total = total.max(1);
            // Atomic `create_new`: from here on the name is ours, and a
            // failure or a cancel removes it (WriteTarget's drop).
            let mut target = this
                .inner
                .roots
                .create_write(&path)
                .map_err(|e| SessionError::Refused(e.to_string()))?;
            let file = target
                .file()
                .ok_or_else(|| SessionError::Failed("the target is not open".to_owned()))?;
            let completed = copy_bounded(&mut source, file, limits.max_export_bytes, |done| {
                let _ = tx.send(0.9 + 0.1 * (done as f64 / total as f64));
                !cancel.is_cancelled()
            })
            .map_err(|e| SessionError::Failed(format!("writing {path}: {e}")))?;
            if !completed {
                return Err(SessionError::Cancelled);
            }
            file.sync_all()
                .map_err(|e| SessionError::Failed(format!("writing {path}: {e}")))?;
            let display = target.display().to_owned();
            target.commit();
            Ok(json!({
                "saved": true,
                "path": display,
                "format": format.extension(),
                "bytes": total,
                "report": report,
            }))
        });
        tokio::pin!(job);
        let token = ctx.and_then(|c| c.meta.get_progress_token());
        let mut last = Instant::now().checked_sub(Duration::from_secs(1));
        let mut sent = -1.0_f64;
        let result = loop {
            tokio::select! {
                biased;
                Some(fraction) = rx.recv() => {
                    let value = (fraction * 1000.0).floor();
                    let due = last.is_none_or(|t| t.elapsed() >= Duration::from_millis(100));
                    if let (Some(token), Some(ctx)) = (token.clone(), ctx)
                        && due
                        && value > sent
                        && !ctx.ct.is_cancelled()
                    {
                        sent = value;
                        last = Some(Instant::now());
                        let _ = ctx
                            .peer
                            .notify_progress(
                                ProgressNotificationParam::new(token, value).with_total(1000.0),
                            )
                            .await;
                    }
                }
                result = &mut job => break result,
            }
        };
        if over.load(Ordering::SeqCst) && matches!(result, Err(SessionError::Cancelled)) {
            return Err(Fail::Tool(export_too_large(limits)));
        }
        Ok(result?)
    }

    fn is_headless(&self, id: &str) -> bool {
        SERVER_COMMANDS.contains(&id) || self.inner.headless.contains(&id)
    }

    /// Runs one validated command. A host command's enabled check and its
    /// run happen in one lock hold ([`run_host_command`]).
    async fn execute(
        &self,
        call: Validated,
        ctx: Option<&RequestContext<RoleServer>>,
    ) -> Result<Value, Fail> {
        let id = call.id.clone();
        if !self.is_headless(&id) {
            return Err(Fail::Tool(format!(
                "`{id}` is not available in the headless server: it needs a window or has no \
                 headless form yet (command_list marks each command's `headless`)"
            )));
        }
        match id.as_str() {
            "file.open" => {
                let path = call.text("path").unwrap_or_default().to_owned();
                self.open_impl(path, ctx).await
            }
            "file.save" => {
                let path = call.text("path").unwrap_or_default().to_owned();
                self.save_impl(
                    SaveParams {
                        path,
                        accept_losses: None,
                    },
                    ctx,
                )
                .await
            }
            _ => match self
                .blocking(ctx, move |session, _| Ok(run_host_command(session, &call)))
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => Err(error.into()),
            },
        }
    }

    async fn command_list_value(&self, p: ListParams) -> Result<Value, Fail> {
        let (has_document, context) = self
            .blocking(None, |s, _| Ok((s.has_document(), s.command_context())))
            .await?;
        let registry = Registry::builtin();
        let listing = registry.list();
        let needle = p.filter.map(|f| f.to_lowercase());
        let mut commands = Vec::new();
        for info in &listing.commands {
            if let Some(needle) = &needle
                && !info.id.to_lowercase().contains(needle)
                && !info.title.to_lowercase().contains(needle)
            {
                continue;
            }
            let headless = self.is_headless(&info.id);
            let reason = if !headless {
                None
            } else if !has_document && info.id != "file.open" {
                Some("No document is open.".to_owned())
            } else {
                registry
                    .check_enabled(&info.id, &context)
                    .err()
                    .map(|e| e.to_string())
            };
            let enabled = headless && reason.is_none();
            if p.enabled_only.unwrap_or(false) && !enabled {
                continue;
            }
            if p.headless_only.unwrap_or(false) && !headless {
                continue;
            }
            let mut value = serde_json::to_value(info).map_err(|e| Fail::Tool(e.to_string()))?;
            if let Some(object) = value.as_object_mut() {
                object.insert("headless".into(), json!(headless));
                object.insert("enabled".into(), json!(enabled));
                object.insert("disabled_reason".into(), json!(reason));
            }
            commands.push(value);
        }
        let families = if p.headless_only.unwrap_or(false) || p.enabled_only.unwrap_or(false) {
            Vec::new()
        } else {
            let mut families = Vec::new();
            for family in &listing.families {
                let mut value =
                    serde_json::to_value(family).map_err(|e| Fail::Tool(e.to_string()))?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("headless".into(), json!(false));
                }
                families.push(value);
            }
            families
        };
        Ok(json!({
            "document_open": has_document,
            "commands": commands,
            "families": families,
        }))
    }

    async fn inspect_value(&self) -> Result<Value, Fail> {
        Ok(self.blocking(None, |s, _| s.inspect()).await?)
    }
}

fn export_too_large(limits: Limits) -> String {
    format!(
        "the export is larger than the limit of {} MiB (raise it with --max-export-mb); nothing \
         was written",
        limits.max_export_bytes >> 20
    )
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[tool_router]
impl AuroraMcp {
    #[tool(
        description = "List the editor's commands with their parameters, whether each can run in \
        this headless server (`headless`) and whether it can run right now (`enabled`, \
        `disabled_reason`). Read-only.",
        annotations(
            title = "List commands",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn command_list(
        &self,
        Parameters(p): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        to_result(self.command_list_value(p).await)
    }

    #[tool(
        description = "Run one command on the open document. Arguments are checked against the \
        command's parameters first: an unknown id or argument name, a wrong type or an out-of-range \
        value is a -32602 error and nothing runs. A command that needs a window is refused with \
        an error saying so. May edit the document (one undo step).",
        annotations(
            title = "Run a command",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn command_run(
        &self,
        Parameters(p): Parameters<RunParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let call = validate_step(&p).map_err(|m| McpError::invalid_params(m, None))?;
        to_result(
            self.execute(call, Some(&ctx))
                .await
                .map(|result| json!({"id": p.id, "ok": true, "result": result})),
        )
    }

    #[tool(
        description = "Run commands in order. NOT atomic: there is no rollback, earlier steps stay \
        applied when a later one fails (use edit.undo once per applied edit, inspecting with \
        doc_inspect). Every step is checked before the first runs, so a bad id or argument is a \
        -32602 error and nothing runs. By default it stops at the first failing step \
        (stop_on_error: false continues). At most 32 steps.",
        annotations(
            title = "Run commands in order",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn command_batch(
        &self,
        Parameters(p): Parameters<BatchParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let max = self.inner.limits.max_batch_steps;
        if p.steps.is_empty() || p.steps.len() > max {
            return Err(McpError::invalid_params(
                format!("`steps` must hold 1 to {max} steps, not {}", p.steps.len()),
                None,
            ));
        }
        let mut calls = Vec::with_capacity(p.steps.len());
        for (index, step) in p.steps.iter().enumerate() {
            calls.push(
                validate_step(step)
                    .map_err(|m| McpError::invalid_params(format!("step {index}: {m}"), None))?,
            );
        }
        let stop = p.stop_on_error.unwrap_or(true);
        let total = calls.len();
        let all_host = calls.iter().all(|c| !SERVER_COMMANDS.contains(&c.id.as_str()));
        let mut results: Vec<Value> = Vec::new();
        let mut failed = 0_usize;
        let mut budget = 0_usize;
        if all_host && calls.iter().all(|c| self.is_headless(&c.id)) {
            // Every step is a host command: run the whole batch in ONE lock
            // hold, so no other request can interleave between its steps.
            let outcome = self
                .blocking(Some(&ctx), move |session, cancel| {
                    let mut results: Vec<Value> = Vec::new();
                    let mut failed = 0_usize;
                    let mut budget = 0_usize;
                    for (index, call) in calls.into_iter().enumerate() {
                        if cancel.is_cancelled() {
                            break;
                        }
                        let id = call.id.clone();
                        let (entry, ok) = step_entry(index, &id, run_host_command(session, &call));
                        failed += usize::from(!ok);
                        budget = budget.saturating_add(entry.to_string().len());
                        if budget > MAX_RESPONSE_BYTES / 2 {
                            results.push(json!({"step": index, "ok": false, "error": "the batch reply budget is spent; this step may have completed"}));
                            failed += 1;
                            break;
                        }
                        results.push(entry);
                        if !ok && stop {
                            break;
                        }
                    }
                    Ok((results, failed))
                })
                .await;
            match outcome {
                Ok((r, f)) => (results, failed) = (r, f),
                Err(error) => return Ok(fail_result(error.to_string())),
            }
        } else {
            // A step that touches files (file.open, file.save) takes the
            // lock in its own pieces, so other requests may interleave
            // BETWEEN steps of such a batch (each step is still atomic).
            for (index, call) in calls.into_iter().enumerate() {
                if ctx.ct.is_cancelled() {
                    break;
                }
                let id = call.id.clone();
                let (entry, ok) = step_entry(index, &id, self.execute(call, Some(&ctx)).await);
                failed += usize::from(!ok);
                budget = budget.saturating_add(entry.to_string().len());
                if budget > MAX_RESPONSE_BYTES / 2 {
                    results.push(json!({"step": index, "ok": false, "error": "the batch reply budget is spent; this step may have completed"}));
                    failed += 1;
                    break;
                }
                results.push(entry);
                if !ok && stop {
                    break;
                }
            }
        }
        let completed = results.len() - failed;
        Ok(ok_json(&json!({
            "completed": completed,
            "failed": failed,
            "not_run": total - results.len(),
            "atomic": false,
            "interleaved_by_other_requests": !all_host,
            "results": results,
        })))
    }

    #[tool(
        description = "Open an image or document file from the read root and make it the session's \
        document, replacing the previous one (unsaved in-memory edits of it are lost). Returns the \
        size, layer count and anything the open could not keep. Needs a read root.",
        annotations(
            title = "Open a file",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn doc_open(
        &self,
        Parameters(p): Parameters<OpenParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        to_result(self.open_impl(p.path, Some(&ctx)).await)
    }

    #[tool(
        description = "The open document as JSON: canvas size, layer tree (top to bottom), undo \
        history, selection. `{\"document\": null}` when none is open. Read-only.",
        annotations(
            title = "Inspect the document",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn doc_inspect(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<CallToolResult, McpError> {
        to_result(self.inspect_value().await)
    }

    #[tool(
        description = "Render the flattened document as a PNG image no larger than max_side \
        (default 1024, at most 2048) on its longest side. Read-only.",
        annotations(
            title = "Render a preview",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn render_preview(
        &self,
        Parameters(p): Parameters<PreviewParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let limits = self.inner.limits;
        let side = p.max_side.unwrap_or(limits.default_preview_side);
        if side == 0 || side > limits.max_preview_side {
            return Err(McpError::invalid_params(
                format!(
                    "`max_side` must be between 1 and {}, not {side}",
                    limits.max_preview_side
                ),
                None,
            ));
        }
        let preview = match self
            .blocking(Some(&ctx), move |s, cancel| s.preview(side, cancel))
            .await
        {
            Ok(preview) => preview,
            Err(error) => return Ok(fail_result(error.to_string())),
        };
        if preview.png.len() > MAX_PREVIEW_PNG_BYTES {
            return Ok(fail_result(format!(
                "the preview PNG is {} bytes; at most {MAX_PREVIEW_PNG_BYTES} are returned; ask \
                 for a smaller max_side",
                preview.png.len()
            )));
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&preview.png);
        let note = json!({
            "width": preview.width,
            "height": preview.height,
            "document_width": preview.document_width,
            "document_height": preview.document_height,
        });
        Ok(CallToolResult::success(vec![
            Content::image(encoded, "image/png"),
            Content::text(note.to_string()),
        ]))
    }

    #[tool(
        description = "Write the document to a NEW file inside the write root; the extension picks \
        the format (png, jpg, tif, psd, aur). It never overwrites: an existing target, a symbolic \
        link at the target, or a path outside the write root is an error. A PSD that would lose or \
        change something is not written unless accept_losses is true; the grouped report is \
        returned either way. Reports progress when the request carries a progress token, and can \
        be cancelled (a partial file is removed). Needs a write root.",
        annotations(
            title = "Save the document to a new file",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn doc_save(
        &self,
        Parameters(p): Parameters<SaveParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        to_result(self.save_impl(p, Some(&ctx)).await)
    }

    #[tool(
        description = "What saving the document as a PSD would keep, change or not save, as a \
        grouped report (needs attention, not saved, changed, from open, kept). Runs the real writer \
        into a sink and writes no file. Read-only.",
        annotations(
            title = "Check PSD compatibility",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn psd_check(
        &self,
        Parameters(_): Parameters<NoParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let outcome = self
            .blocking(Some(&ctx), |s, cancel| {
                if !s.has_document() {
                    return Err(SessionError::NoDocument);
                }
                s.psd_check(cancel)
            })
            .await
            .map_err(Fail::from);
        to_result(outcome)
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

impl ServerHandler for AuroraMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("aurora", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self.tool_router.list_all();
        serde_json::from_value(
            json!({"resultType": "complete", "ttlMs": 600_000, "cacheScope": "private", "tools": tools}),
        )
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // An argument the tool does not declare is an error before anything
        // runs, whatever the schema's own deserialiser would do with it.
        if let Some(tool) = self.tool_router.get(&request.name) {
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object);
            if let Some(args) = &request.arguments
                && let Some(key) = args
                    .keys()
                    .find(|k| properties.is_none_or(|p| !p.contains_key(*k)))
            {
                return Err(McpError::invalid_params(
                    format!("unknown argument `{key}` for {}", request.name),
                    None,
                ));
            }
        }
        if let Some(token) = context.meta.get_progress_token()
            && serde_json::to_string(&token).map_or(true, |t| t.len() > MAX_PROGRESS_TOKEN_BYTES)
        {
            return Err(McpError::invalid_params(
                format!("the progress token is longer than {MAX_PROGRESS_TOKEN_BYTES} bytes"),
                None,
            ));
        }
        // Wrong types and missing arguments are also protocol errors, not
        // tool errors: parse them here, the way the tool will, first.
        check_arguments(&request.name, request.arguments.clone())
            .map_err(|m| McpError::invalid_params(format!("{}: {m}", request.name), None))?;
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        serde_json::from_value(json!({"resultType": "complete", "ttlMs": 600_000, "cacheScope": "private", "resources": [
            {"uri": DOCUMENT, "name": "document", "title": "Open document", "mimeType": "application/json", "description": "Live doc_inspect state."},
            {"uri": COMMANDS, "name": "commands", "title": "Command catalog", "mimeType": "application/json", "description": "Live command_list."},
        ]}))
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let value = match request.uri.as_str() {
            DOCUMENT => self.inspect_value().await,
            COMMANDS => self.command_list_value(ListParams::default()).await,
            uri => {
                return Err(McpError::resource_not_found(
                    format!("unknown resource `{uri}`"),
                    None,
                ));
            }
        };
        let text = match value {
            Ok(value) => json_text(&value).map_err(|m| McpError::internal_error(m, None))?,
            Err(Fail::Tool(m)) => {
                return Err(McpError::internal_error(m, None));
            }
        };
        let result: ReadResourceResult = serde_json::from_value(json!({
            "resultType": "complete",
            "ttlMs": 0,
            "cacheScope": "private",
            "contents": [{"uri": request.uri, "mimeType": "application/json", "text": text}],
        }))
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(result.into())
    }
}

/// Parses `arguments` as `tool` will, so a mistake is a `-32602` before
/// anything runs.
fn check_arguments(tool: &str, arguments: Option<Map<String, Value>>) -> Result<(), String> {
    fn parse<T: serde::de::DeserializeOwned>(
        arguments: Option<Map<String, Value>>,
    ) -> Result<(), String> {
        serde_json::from_value::<T>(Value::Object(arguments.unwrap_or_default()))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    match tool {
        "command_list" => parse::<ListParams>(arguments),
        "command_run" => parse::<RunParams>(arguments),
        "command_batch" => parse::<BatchParams>(arguments),
        "doc_open" => parse::<OpenParams>(arguments),
        "doc_inspect" | "psd_check" => parse::<NoParams>(arguments),
        "render_preview" => parse::<PreviewParams>(arguments),
        "doc_save" => parse::<SaveParams>(arguments),
        _ => Ok(()),
    }
}

/// The tools' names, for tests and docs.
#[must_use]
pub fn tool_names() -> Vec<String> {
    AuroraMcp::tool_router()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}
