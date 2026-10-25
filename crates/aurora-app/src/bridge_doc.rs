//! The document half of the bridge handlers (task 160): commands, inspect,
//! preview, PSD check, open and save over the **live** document, through the
//! same code the headless MCP session uses (`mcp_session`).

use std::io::Write as _;

use aurora_automation::bridge::protocol::CallError;
use aurora_automation::header;
use aurora_commands::{ArgValue, Args, ParamKind};
use base64::Engine as _;
use serde_json::{Map, Value, json};

use crate::bridge_app::{BLOCKED_COMMANDS, Job, Pending};
use crate::command_registry::{self, TypedError};
use crate::mcp_session::{
    self, ExportKind, ExportParts, ExportResult, Failure, InspectView, composite_document_cpu,
};
use crate::{ActivatedCommand, App, activate_command};

/// Largest file `doc.open` copies.
const MAX_OPEN_BYTES: u64 = 512 << 20;
/// Largest file `doc.save` writes.
const MAX_EXPORT_BYTES: u64 = 1 << 30;
/// Largest preview side.
const MAX_PREVIEW_SIDE: u32 = 2048;

pub(crate) fn bad(message: impl Into<String>) -> CallError {
    CallError::new("bad_request", message)
}

fn failure(error: Failure) -> CallError {
    match error {
        Failure::NoDocument => CallError::new("failed", "no document is open"),
        Failure::NotAvailable(m) => CallError::new("unavailable", m),
        Failure::Refused(m) => CallError::new("refused", m),
        Failure::Failed(m) => CallError::new("failed", m),
        Failure::Cancelled => CallError::new("cancelled", "cancelled"),
    }
}

/// A JSON object as registry arguments.
fn args_from(map: Option<&Map<String, Value>>) -> Result<Args, CallError> {
    let mut args = Args::new();
    if let Some(map) = map {
        for (name, value) in map {
            let converted = match value {
                Value::Bool(b) => ArgValue::Bool(*b),
                Value::String(s) => ArgValue::Text(s.clone()),
                Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                    (Some(i), _) => ArgValue::Int(i),
                    (None, Some(f)) => ArgValue::Number(f),
                    _ => return Err(bad(format!("`{name}` is not a usable number"))),
                },
                _ => return Err(bad(format!("`{name}` must be a boolean, a number or text"))),
            };
            args.insert(name.clone(), converted);
        }
    }
    Ok(args)
}

/// Refuses every native file dialog: the bridge never blocks the UI thread
/// on one and never lets a command choose a path.
struct NoDialogs;

impl crate::FileDialogAccess for NoDialogs {
    fn pick_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }

    fn save_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
}

/// Commands the bridge never runs: quitting and the bridge's own control
/// (blocked by id), the file commands (`doc_open` / `doc_save` reach files
/// through the roots) and anything with a file-path parameter.
pub(crate) fn command_gate(id: &str) -> Result<(), CallError> {
    if BLOCKED_COMMANDS.contains(&id) {
        return Err(CallError::new(
            "refused",
            format!("`{id}` is not available over the bridge"),
        ));
    }
    if id == crate::COMMAND_FILE_OPEN || id == crate::COMMAND_FILE_SAVE {
        return Err(CallError::new(
            "refused",
            "use doc_open and doc_save: files are only reached through the bridge's roots",
        ));
    }
    if let Some(spec) = command_registry::registry().get(id)
        && spec
            .params
            .iter()
            .any(|p| matches!(p.kind, ParamKind::Path { .. }))
    {
        return Err(CallError::new(
            "refused",
            format!("`{id}` takes a file path; the bridge does not pass paths to commands"),
        ));
    }
    Ok(())
}

impl App {
    pub(crate) fn bridge_command_context(&self) -> Value {
        let facts = command_registry::facts(&self.menu_context());
        json!({
            "can_undo": facts.can_undo,
            "can_redo": facts.can_redo,
            "delete_allowed": facts.delete_allowed,
            "quick_mask": facts.quick_mask,
            "clipped": facts.clipped,
            "clip_refused": facts.clip_refused,
        })
    }

    /// `command.run {id, args}`: validated again here (the proxy's check is
    /// not trusted), checked enabled against the live state, then run
    /// through [`activate_command`] / [`App::run_activated`], the path the
    /// palette and the menus use.
    pub(crate) fn bridge_command_run(&mut self, job: &Job) -> Result<Value, CallError> {
        let id = job
            .param("id")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("`id` is required"))?
            .to_owned();
        let args = args_from(job.param("args").and_then(Value::as_object))?;
        let registry = command_registry::registry();
        let validated = registry
            .validate_args(&id, &args)
            .map_err(|e| bad(e.to_string()))?;
        command_gate(&id)?;
        let context = command_registry::facts(&self.menu_context());
        if let Err(reason) = registry.check_enabled(&id, &context) {
            return Err(CallError::new(
                "disabled",
                format!("`{id}` is not enabled: {reason}"),
            ));
        }
        let picked = match command_registry::activated_from_args(&id, &args) {
            Ok(command) => Some(command),
            Err(TypedError::NoTypedForm) => {
                activate_command(&mut self.workspace, &mut self.focus, &id, &mut NoDialogs)
            }
            Err(TypedError::Invalid(e)) => return Err(bad(e.to_string())),
            Err(TypedError::Refused(m)) => return Err(CallError::new("refused", m)),
        };
        // Native dialogs are refused above, so no path command can appear.
        if matches!(
            picked,
            Some(ActivatedCommand::OpenFile(_) | ActivatedCommand::SaveFile(_))
        ) {
            return Err(CallError::new(
                "refused",
                "file commands are not run over the bridge",
            ));
        }
        let _ = validated;
        self.run_activated(picked);
        self.push_accessibility();
        self.relayout_after_gallery();
        self.needs_redraw = true;
        Ok(json!({"id": id, "ok": true, "dialog": self.bridge_modal()}))
    }

    pub(crate) fn bridge_doc_inspect(&mut self) -> Value {
        let notes: Vec<String> = self.doc.import_losses.clone().unwrap_or_default();
        mcp_session::inspect_json(InspectView {
            name: &self.doc.name,
            format: "live",
            layers: &self.doc.layers,
            active: self.doc.active_layer,
            canvas: self.doc.canvas_size,
            undo_order: &mut self.doc.undo_order,
            selection: &self.doc.selection,
            notes: &notes,
        })
    }

    pub(crate) fn bridge_doc_preview(&mut self, job: &Job) -> Result<Value, CallError> {
        let side = match job.param("max_side") {
            None => 1024,
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=MAX_PREVIEW_SIDE).contains(n))
                .ok_or_else(|| bad(format!("`max_side` must be 1 to {MAX_PREVIEW_SIDE}")))?,
        };
        let max_pixels = self.bridge.as_ref().map_or(0, |b| b.max_pixels);
        let canvas = self.doc.canvas_size;
        let Some(store) = self.doc.tile_store.as_mut() else {
            return Err(CallError::new("failed", "the document has no tile store"));
        };
        let image = composite_document_cpu(
            &self.doc.layers,
            store,
            canvas,
            max_pixels,
            &|| false,
            &mut |_| {},
        )
        .map_err(failure)?;
        let preview = mcp_session::preview_from_image(image, side).map_err(failure)?;
        Ok(json!({
            "png_base64": base64::engine::general_purpose::STANDARD.encode(&preview.png),
            "width": preview.width,
            "height": preview.height,
            "document_width": preview.document_width,
            "document_height": preview.document_height,
        }))
    }

    fn with_export_parts<T>(
        &mut self,
        run: impl FnOnce(ExportParts<'_>) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let losses = self.doc.import_losses.clone().unwrap_or_default();
        let Some(store) = self.doc.tile_store.as_mut() else {
            return Err(Failure::Failed("the document has no tile store".to_owned()));
        };
        run(ExportParts {
            name: &self.doc.name,
            layers: &self.doc.layers,
            history: &self.doc.history,
            canvas: self.doc.canvas_size,
            skipped: &self.doc.skipped_tiles,
            import_losses: (!losses.is_empty()).then_some(losses.as_slice()),
            selection: &self.doc.selection,
            colour: &self.doc.colour,
            id: self.doc.id,
            store,
        })
    }

    pub(crate) fn bridge_doc_psd_check(&mut self) -> Result<Value, CallError> {
        self.with_export_parts(|parts| mcp_session::psd_check_document(parts, &|| false))
            .map_err(failure)
    }

    /// `doc.save {path, accept_losses}`: the document is exported to the
    /// bridge's private scratch, then copied into a NEW file inside a root
    /// (`create_new`; an existing target is never touched).
    pub(crate) fn bridge_doc_save(&mut self, job: &Job) -> Result<Value, CallError> {
        let path = job
            .param("path")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("`path` is required"))?
            .to_owned();
        let accept = job
            .param("accept_losses")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let extension = std::path::Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = match extension.as_str() {
            "png" => ExportKind::Png,
            "jpg" | "jpeg" => ExportKind::Jpeg,
            "tif" | "tiff" => ExportKind::Tiff,
            "psd" => ExportKind::Psd,
            "aur" => ExportKind::Aur,
            _ => return Err(bad("the extension must be png, jpg, tif, psd or aur")),
        };
        let (max_pixels, scratch_dir, directory) = {
            let Some(bridge) = self.bridge.as_mut() else {
                return Err(CallError::new("unavailable", "the bridge is off"));
            };
            bridge
                .roots
                .check_write(&path)
                .map_err(|e| CallError::new("refused", e.to_string()))?;
            let n = bridge.next_counter();
            let dir = bridge.scratch.path().join(format!("export-{n}"));
            (bridge.max_pixels, bridge.scratch.path().to_path_buf(), dir)
        };
        let _ = scratch_dir;
        std::fs::create_dir(&directory)
            .map_err(|e| CallError::new("failed", format!("scratch: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
        }
        let out = directory.join(format!(
            "out.{}",
            match kind {
                ExportKind::Png => "png",
                ExportKind::Jpeg => "jpg",
                ExportKind::Tiff => "tif",
                ExportKind::Psd => "psd",
                ExportKind::Aur => "aur",
            }
        ));
        let result = self
            .with_export_parts(|parts| {
                mcp_session::export_document(
                    parts,
                    kind,
                    accept,
                    &out,
                    (max_pixels, MAX_EXPORT_BYTES),
                    &|| false,
                    &mut |_| {},
                )
            })
            .map_err(failure);
        let answer = (|| {
            let report = match result? {
                ExportResult::NeedsConfirmation(report) => {
                    return Ok(json!({
                        "saved": false,
                        "needs_confirmation": true,
                        "message": "This save would lose or change something. Nothing was written. Review the report and call doc_save again with accept_losses: true to write it anyway.",
                        "report": report,
                    }));
                }
                ExportResult::Written(report) => report,
            };
            let bridge = self
                .bridge
                .as_ref()
                .ok_or_else(|| CallError::new("unavailable", "the bridge is off"))?;
            let total = commit_export(&bridge.roots, &out, &path)?;
            Ok(
                json!({"saved": true, "path": path, "format": extension, "bytes": total, "report": report}),
            )
        })();
        let _ = std::fs::remove_dir_all(&directory);
        answer
    }

    /// `doc.open {path}`: confined by the bridge's roots, copied to private
    /// scratch (header-checked on the copy), opened through the app's own
    /// background open; the answer comes when the open has finished.
    pub(crate) fn bridge_doc_open(&mut self, job: Job) -> Option<(Job, Result<Value, CallError>)> {
        match self.bridge_stage_open(&job) {
            Ok(staged) => {
                self.open_file(&staged);
                if let Some(bridge) = self.bridge.as_mut() {
                    bridge.pending.push(Pending::Open {
                        job,
                        since: std::time::Instant::now(),
                    });
                }
                self.needs_redraw = true;
                None
            }
            Err(error) => Some((job, Err(error))),
        }
    }

    fn bridge_stage_open(&mut self, job: &Job) -> Result<std::path::PathBuf, CallError> {
        let path = job
            .param("path")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("`path` is required"))?
            .to_owned();
        if self.open_worker.pending_path().is_some() {
            return Err(CallError::new("busy", "another file is still opening"));
        }
        let Some(bridge) = self.bridge.as_mut() else {
            return Err(CallError::new("unavailable", "the bridge is off"));
        };
        if let Some(old) = bridge.staged.take() {
            let _ = std::fs::remove_dir_all(old);
        }
        let n = bridge.next_counter();
        let dir = bridge.scratch.path().join(format!("open-{n}"));
        bridge.staged = Some(dir.clone());
        stage_open_file(&bridge.roots, &dir, &path, bridge.max_pixels)
    }
}

/// Confines `path` to the read roots, checks its header, and copies it into
/// the new private directory `dir` (keeping its file name), checking the
/// header again on the copy, which is what gets decoded.
pub(crate) fn stage_open_file(
    roots: &aurora_automation::Roots,
    dir: &std::path::Path,
    path: &str,
    max_pixels: u64,
) -> Result<std::path::PathBuf, CallError> {
    let mut read = roots
        .open_read(path, MAX_OPEN_BYTES)
        .map_err(|e| CallError::new("refused", e.to_string()))?;
    check_header(&mut read.file, max_pixels)?;
    std::io::Seek::seek(&mut read.file, std::io::SeekFrom::Start(0))
        .map_err(|e| CallError::new("failed", e.to_string()))?;
    std::fs::create_dir(dir).map_err(|e| CallError::new("failed", format!("scratch: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    // The copy keeps the file's own name (the tab and title show it) but
    // never anything from the caller's path.
    let name: String = read
        .name
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(120)
        .collect();
    let staged = dir.join(if name.is_empty() {
        "document".to_owned()
    } else {
        name
    });
    let mut out = std::fs::OpenOptions::new();
    out.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        out.mode(0o600);
    }
    let mut out = out
        .open(&staged)
        .map_err(|e| CallError::new("failed", format!("scratch file: {e}")))?;
    aurora_automation::roots::copy_bounded(&mut read.file, &mut out, MAX_OPEN_BYTES, |_| true)
        .map_err(|e| CallError::new("failed", format!("reading {}: {e}", read.name)))?;
    out.flush()
        .map_err(|e| CallError::new("failed", e.to_string()))?;
    drop(out);
    let mut copy =
        std::fs::File::open(&staged).map_err(|e| CallError::new("failed", e.to_string()))?;
    check_header(&mut copy, max_pixels)?;
    mcp_session::HeadlessSession::check_aur_header(&staged, max_pixels).map_err(failure)?;
    Ok(staged)
}

/// Copies the finished export `out` into a NEW file `path` inside a write
/// root (`create_new`; an existing target is never touched; a failure
/// removes the file this call created).
pub(crate) fn commit_export(
    roots: &aurora_automation::Roots,
    out: &std::path::Path,
    path: &str,
) -> Result<u64, CallError> {
    let mut source = std::fs::File::open(out)
        .map_err(|e| CallError::new("failed", format!("reading the export: {e}")))?;
    let total = source.metadata().map_or(0, |m| m.len());
    if total > MAX_EXPORT_BYTES {
        return Err(CallError::new(
            "refused",
            "the export is larger than the limit",
        ));
    }
    let mut target = roots
        .create_write(path)
        .map_err(|e| CallError::new("refused", e.to_string()))?;
    let file = target
        .file()
        .ok_or_else(|| CallError::new("failed", "the target is not open"))?;
    aurora_automation::roots::copy_bounded(&mut source, file, MAX_EXPORT_BYTES, |_| true)
        .map_err(|e| CallError::new("failed", format!("writing {path}: {e}")))?;
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|e| CallError::new("failed", format!("writing {path}: {e}")))?;
    target.commit();
    Ok(total)
}

fn check_header(
    file: &mut (impl std::io::Read + std::io::Seek),
    max_pixels: u64,
) -> Result<(), CallError> {
    match header::dimensions(file) {
        Err(message) => Err(CallError::new("refused", message)),
        Ok(Some(d)) if d.width == 0 || d.height == 0 => Err(CallError::new(
            "refused",
            format!("the {} header declares an empty picture", d.format),
        )),
        Ok(Some(d)) if d.pixels() > max_pixels => Err(CallError::new(
            "refused",
            format!(
                "the {} header declares {} x {} px; the bridge opens at most {} Mpx (raise it with --mcp-bridge-max-mpx)",
                d.format,
                d.width,
                d.height,
                max_pixels / 1_000_000
            ),
        )),
        Ok(_) => Ok(()),
    }
}
