//! The bridge host (task 160): the same MCP tools, carried out by the
//! running desktop app over the loopback connection.
//!
//! [`BridgeSession`] implements [`Session`] by forwarding each operation to
//! the app (`aurora-automation`'s bridge client). The tools above it are
//! unchanged: the registry validates arguments before anything is sent, the
//! roots gate paths here as well as in the app, and the hints stay honest.
//! The connection is made lazily, from the app's discovery file, and made
//! again after it drops; a read-only call is retried once on a fresh
//! connection, anything that changes state never is (the reply says the
//! operation may have completed).

use std::path::PathBuf;
use std::time::Duration;

use aurora_automation::bridge::client::{BridgeClient, ClientError};
use aurora_automation::bridge::discovery::{self, Discovery, DiscoveryError};
use aurora_commands::{ArgValue, CommandContext, Registry, Validated};
use base64::Engine as _;
use serde_json::{Map, Value, json};

use crate::session::{Cancel, ExportFormat, ExportOutcome, Preview, Session, SessionError};

/// Longer than the app's own 60 s wait, so its message is the one we see.
const CALL_TIMEOUT: Duration = Duration::from_secs(65);

/// Commands never offered over the bridge (the app refuses them too).
const BLOCKED: &[&str] = &["app.quit", "automation.disconnect"];

/// Where the app's discovery record comes from.
#[derive(Debug, Clone)]
pub enum DiscoverySource {
    /// A specific discovery file.
    File(PathBuf),
    /// The newest live app in this directory.
    Dir(PathBuf),
}

impl DiscoverySource {
    /// The platform default directory.
    ///
    /// # Errors
    /// No default directory exists on this platform.
    pub fn default_dir() -> Result<Self, String> {
        discovery::default_dir().map(Self::Dir).ok_or_else(|| {
            "no default discovery directory on this platform; pass --discovery".to_owned()
        })
    }

    fn resolve(&self) -> Result<Discovery, DiscoveryError> {
        match self {
            Self::File(path) => discovery::read_file(path),
            Self::Dir(dir) => discovery::find_running(dir).map(|(_, record)| record),
        }
    }
}

/// The bridge host.
#[derive(Debug)]
pub struct BridgeSession {
    source: DiscoverySource,
    client: Option<BridgeClient>,
    commands: &'static [&'static str],
}

impl BridgeSession {
    #[must_use]
    pub fn new(source: DiscoverySource) -> Self {
        let ids: Vec<&'static str> = Registry::builtin()
            .list()
            .commands
            .iter()
            .map(|c| &*Box::leak(c.id.clone().into_boxed_str()))
            .filter(|id| !BLOCKED.contains(id))
            .collect();
        Self {
            source,
            client: None,
            commands: Box::leak(ids.into_boxed_slice()),
        }
    }

    fn connect(&mut self) -> Result<(), SessionError> {
        if self.client.is_some() {
            return Ok(());
        }
        let info = self
            .source
            .resolve()
            .map_err(|e| SessionError::Failed(e.to_string()))?;
        let client =
            BridgeClient::connect(&info).map_err(|e| SessionError::Failed(e.to_string()))?;
        self.client = Some(client);
        Ok(())
    }

    /// One call. `read_only` calls get one retry on a fresh connection.
    fn call(
        &mut self,
        method: &str,
        params: &Value,
        read_only: bool,
    ) -> Result<Value, SessionError> {
        let attempts = if read_only { 2 } else { 1 };
        let mut last = SessionError::Failed("no connection".to_owned());
        for _ in 0..attempts {
            self.connect()?;
            let Some(client) = self.client.as_mut() else {
                continue;
            };
            match client.call(method, params.clone(), CALL_TIMEOUT) {
                Ok(value) => return Ok(value),
                Err(ClientError::App(error)) => return Err(map_app_error(&error)),
                Err(error @ (ClientError::Transport(_) | ClientError::Timeout(_))) => {
                    self.client = None;
                    last = SessionError::Failed(error.to_string());
                    if matches!(error, ClientError::Timeout(_)) {
                        break;
                    }
                }
                Err(error @ ClientError::Connect(_)) => {
                    // Nothing was sent (or the send failed): safe to retry once.
                    self.client = None;
                    last = SessionError::Failed(error.to_string());
                }
            }
        }
        Err(last)
    }
}

fn map_app_error(error: &aurora_automation::bridge::protocol::CallError) -> SessionError {
    match error.kind.as_str() {
        "unavailable" => SessionError::NotAvailable(error.message.clone()),
        "failed" | "timeout" | "disconnected" | "busy" => {
            SessionError::Failed(format!("{}: {}", error.kind, error.message))
        }
        // modal_dialog, palette_open, refused, disabled, rate_limited,
        // bad_request, unknown_method, ...
        _ => SessionError::Refused(format!("{}: {}", error.kind, error.message)),
    }
}

fn arg_json(value: &ArgValue) -> Value {
    match value {
        ArgValue::Bool(b) => json!(b),
        ArgValue::Int(i) => json!(i),
        ArgValue::Number(n) => json!(n),
        ArgValue::Text(t) => json!(t),
    }
}

fn flag(value: &Value, name: &str) -> bool {
    value.get(name).and_then(Value::as_bool).unwrap_or(false)
}

impl Session for BridgeSession {
    fn is_bridge(&self) -> bool {
        true
    }

    fn open(
        &mut self,
        _staged: &std::path::Path,
        _name: &str,
        _max_pixels: u64,
        _cancel: &Cancel,
    ) -> Result<Value, SessionError> {
        Err(SessionError::NotAvailable(
            "a bridge session opens files through the app (doc_open)".to_owned(),
        ))
    }

    fn open_path(
        &mut self,
        absolute: &str,
        _max_pixels: u64,
        _cancel: &Cancel,
    ) -> Result<Value, SessionError> {
        self.call("doc.open", &json!({"path": absolute}), false)
    }

    fn save_path(
        &mut self,
        absolute: &str,
        accept_losses: bool,
        _cancel: &Cancel,
    ) -> Result<Value, SessionError> {
        self.call(
            "doc.save",
            &json!({"path": absolute, "accept_losses": accept_losses}),
            false,
        )
    }

    fn inspect(&mut self) -> Result<Value, SessionError> {
        self.call("doc.inspect", &json!({}), true)
    }

    fn has_document(&self) -> bool {
        // The running app always has a document (an untitled one at least).
        true
    }

    fn command_context(&mut self) -> CommandContext {
        let Ok(value) = self.call("command.context", &json!({}), true) else {
            return CommandContext::default();
        };
        CommandContext {
            can_undo: flag(&value, "can_undo"),
            can_redo: flag(&value, "can_redo"),
            delete_allowed: flag(&value, "delete_allowed"),
            quick_mask: flag(&value, "quick_mask"),
            clipped: flag(&value, "clipped"),
            clip_refused: flag(&value, "clip_refused"),
        }
    }

    fn headless_commands(&self) -> &'static [&'static str] {
        self.commands
    }

    fn run_command(&mut self, call: &Validated) -> Result<Value, SessionError> {
        let args: Map<String, Value> = call
            .args
            .iter()
            .map(|(name, value)| (name.clone(), arg_json(value)))
            .collect();
        self.call("command.run", &json!({"id": call.id, "args": args}), false)
    }

    fn preview(&mut self, max_side: u32, _cancel: &Cancel) -> Result<Preview, SessionError> {
        let value = self.call("doc.preview", &json!({"max_side": max_side}), true)?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(
                value
                    .get("png_base64")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .map_err(|e| SessionError::Failed(format!("the app's preview is not valid: {e}")))?;
        let n = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0)
        };
        Ok(Preview {
            png,
            width: n("width"),
            height: n("height"),
            document_width: n("document_width"),
            document_height: n("document_height"),
        })
    }

    fn export(
        &mut self,
        _format: ExportFormat,
        _accept_losses: bool,
        _out: &std::path::Path,
        _cancel: &Cancel,
        _progress: &mut dyn FnMut(f64),
    ) -> Result<ExportOutcome, SessionError> {
        Err(SessionError::NotAvailable(
            "a bridge session saves through the app (doc_save)".to_owned(),
        ))
    }

    fn psd_check(&mut self, _cancel: &Cancel) -> Result<Value, SessionError> {
        self.call("doc.psd_check", &json!({}), true)
    }

    fn ui_call(
        &mut self,
        method: &str,
        params: Value,
        _cancel: &Cancel,
    ) -> Result<Value, SessionError> {
        let read_only = matches!(method, "ui.inspect" | "ui.screenshot")
            || (method == "ui.dialog"
                && params.get("action").and_then(Value::as_str) != Some("choose"))
            || (method == "ui.menu"
                && params.get("action").and_then(Value::as_str) != Some("activate"));
        self.call(method, &params, read_only)
    }
}
