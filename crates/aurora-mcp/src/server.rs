//! The MCP tools, resources and their security rules.
//!
//! Tool names are `snake_case` (dots are not valid in every client's
//! grammar). Every tool carries all four annotation hints; they are honest
//! hints, not permissions. Arguments are strict: an unknown tool argument,
//! a wrong type, an unknown command id or command argument is a JSON-RPC
//! `-32602` error returned **before anything runs**. A command that was
//! understood but cannot run (disabled, needs a window, a file problem) is
//! a tool result with `isError: true`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
use crate::roots::{PathError, Roots, copy_bounded};
use crate::session::{Cancel, ExportFormat, ExportOutcome, Session, SessionError};

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
    session: Mutex<Box<dyn Session>>,
    roots: Roots,
    limits: Limits,
    scratch: tempfile::TempDir,
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

impl AuroraMcp {
    /// Builds a server over `session` with the granted `roots`.
    ///
    /// # Errors
    /// The private scratch directory could not be created.
    pub fn new(
        session: Box<dyn Session>,
        roots: Roots,
        limits: Limits,
        scratch_parent: Option<&Path>,
    ) -> std::io::Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("aurora-mcp-");
        let scratch = match scratch_parent {
            Some(parent) => builder.tempdir_in(parent)?,
            None => builder.tempdir()?,
        };
        let mut tool_router = Self::tool_router();
        for route in tool_router.map.values_mut() {
            let mut schema = (*route.attr.input_schema).clone();
            schema.insert("additionalProperties".into(), json!(false));
            route.attr.input_schema = Arc::new(schema);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                session: Mutex::new(session),
                roots,
                limits,
                scratch,
                counter: AtomicU64::new(1),
            }),
            tool_router,
        })
    }

    fn scratch_path(&self, extension: &str) -> PathBuf {
        let n = self.inner.counter.fetch_add(1, Ordering::Relaxed);
        self.inner.scratch.path().join(format!("{n}.{extension}"))
    }

    /// Runs `f` on the session on a blocking thread. `ctx`'s cancellation
    /// becomes the [`Cancel`] flag `f` polls.
    async fn blocking<T: Send + 'static>(
        &self,
        ctx: Option<&RequestContext<RoleServer>>,
        f: impl FnOnce(&mut dyn Session, &Cancel) -> Result<T, SessionError> + Send + 'static,
    ) -> Result<T, SessionError> {
        let cancel = Cancel::new();
        let watcher = ctx.map(|ctx| {
            let token = ctx.ct.clone();
            let flag = cancel.clone();
            tokio::spawn(async move {
                token.cancelled().await;
                flag.cancel();
            })
        });
        let inner = Arc::clone(&self.inner);
        let flag = cancel.clone();
        let joined = tokio::task::spawn_blocking(move || {
            // A panic poisons the lock; the session is used again rather
            // than failing every later call (a release build aborts on a
            // panic anyway, so this is only reachable in dev builds).
            let mut guard = inner.session.lock().unwrap_or_else(PoisonError::into_inner);
            f(guard.as_mut(), &flag)
        })
        .await;
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        joined.unwrap_or_else(|e| Err(SessionError::Failed(format!("the operation failed: {e}"))))
    }

    // ----- shared implementations -----

    async fn open_impl(
        &self,
        path: String,
        ctx: Option<&RequestContext<RoleServer>>,
    ) -> Result<Value, Fail> {
        let this = self.clone();
        let staged_dir = self.inner.scratch.path().to_path_buf();
        let counter = self.inner.counter.fetch_add(1, Ordering::Relaxed);
        let max = self.inner.limits.max_open_bytes;
        // Stage: read the confined file once into the private scratch
        // directory, then open that copy. The host never receives, and
        // never re-resolves, a path of the caller's.
        let result = self
            .blocking(ctx, move |session, cancel| {
                let mut read = this
                    .inner
                    .roots
                    .open_read(&path, max)
                    .map_err(|e| SessionError::Refused(e.to_string()))?;
                let extension = safe_extension(&read.name);
                let staged = ScratchFile(staged_dir.join(format!("open-{counter}.{extension}")));
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&staged.0)
                    .map_err(|e| SessionError::Failed(format!("scratch file: {e}")))?;
                let completed =
                    copy_bounded(&mut read.file, &mut out, max, |_| !cancel.is_cancelled())
                        .map_err(|e| SessionError::Failed(format!("reading {}: {e}", read.name)))?;
                if !completed {
                    return Err(SessionError::Cancelled);
                }
                out.flush()
                    .map_err(|e| SessionError::Failed(format!("scratch file: {e}")))?;
                drop(out);
                session.open(&staged.0, &read.name, cancel)
            })
            .await?;
        Ok(result)
    }

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
        // Refuse a bad or existing target before the expensive export.
        self.inner.roots.check_write(&params.path)?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
        let this = self.clone();
        let scratch = ScratchFile(self.scratch_path(format.extension()));
        let accept = params.accept_losses.unwrap_or(false);
        let path = params.path;
        let max_bytes = self.inner.limits.max_export_bytes;
        let job = self.blocking(ctx, move |session, cancel| {
            let outcome = session.export(format, accept, &scratch.0, cancel, &mut |f| {
                let _ = tx.send((f.clamp(0.0, 1.0)) * 0.9);
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
            let mut source = std::fs::File::open(&scratch.0)
                .map_err(|e| SessionError::Failed(format!("reading the export: {e}")))?;
            let total = source.metadata().map_or(0, |m| m.len()).max(1);
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
            let completed = copy_bounded(&mut source, file, max_bytes, |done| {
                let _ = tx.send(0.9 + 0.1 * (done as f64 / total as f64));
                !cancel.is_cancelled()
            })
            .map_err(|e| SessionError::Failed(format!("writing {path}: {e}")))?;
            if !completed {
                return Err(SessionError::Cancelled);
            }
            file.sync_all()
                .map_err(|e| SessionError::Failed(format!("writing {path}: {e}")))?;
            let bytes = total;
            let display = target.display().to_owned();
            target.commit();
            Ok(json!({
                "saved": true,
                "path": display,
                "format": format.extension(),
                "bytes": bytes,
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
        Ok(result?)
    }

    fn is_headless(&self, id: &str) -> bool {
        if SERVER_COMMANDS.contains(&id) {
            return true;
        }
        self.inner
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .headless_commands()
            .contains(&id)
    }

    /// Runs one validated command call.
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
        let (has_document, context) = self
            .blocking(None, |s, _| Ok((s.has_document(), s.command_context())))
            .await?;
        if !has_document && id != "file.open" {
            return Err(Fail::Tool(SessionError::NoDocument.to_string()));
        }
        if let Err(reason) = Registry::builtin().check_enabled(&id, &context) {
            return Err(Fail::Tool(format!("`{id}` is not enabled: {reason}")));
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
            _ => Ok(self
                .blocking(ctx, move |session, _| session.run_command(&call))
                .await?),
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
        let mut results: Vec<Value> = Vec::new();
        let mut failed = 0_usize;
        let mut budget = 0_usize;
        for (index, call) in calls.into_iter().enumerate() {
            if ctx.ct.is_cancelled() {
                break;
            }
            let id = call.id.clone();
            let outcome = self.execute(call, Some(&ctx)).await;
            let entry = match outcome {
                Ok(result) => json!({"step": index, "id": id, "ok": true, "result": result}),
                Err(Fail::Tool(message)) => {
                    failed += 1;
                    json!({"step": index, "id": id, "ok": false, "error": message})
                }
            };
            let failed_step = entry.get("ok") == Some(&json!(false));
            budget = budget.saturating_add(entry.to_string().len());
            if budget > MAX_RESPONSE_BYTES / 2 {
                results.push(json!({
                    "step": index,
                    "ok": false,
                    "error": "the batch reply budget is spent; this step may have completed",
                }));
                failed += 1;
                break;
            }
            results.push(entry);
            if failed_step && stop {
                break;
            }
        }
        let completed = results.len() - failed;
        Ok(ok_json(&json!({
            "completed": completed,
            "failed": failed,
            "not_run": p.steps.len() - results.len(),
            "atomic": false,
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
