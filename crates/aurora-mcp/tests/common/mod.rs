//! A mock document host and a JSON-RPC driver for the protocol tests.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aurora_commands::{CommandContext, Validated};
use aurora_mcp::{
    Cancel, Config, ExportFormat, ExportOutcome, Limits, Preview, Roots, Session, SessionError,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// What the mock saw.
#[derive(Debug, Default)]
pub struct Seen {
    pub opened: Mutex<Vec<(String, Vec<u8>)>>,
    pub commands: Mutex<Vec<String>>,
    pub exports: AtomicUsize,
}

pub struct Mock {
    pub seen: Arc<Seen>,
    pub document: bool,
    /// The export blocks (polling the cancel flag) until cancelled.
    pub hang_on_export: bool,
    /// The next PSD export reports losses.
    pub lossy_psd: bool,
    pub export_bytes: Vec<u8>,
}

impl Mock {
    pub fn new() -> (Self, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        (
            Self {
                seen: Arc::clone(&seen),
                document: false,
                hang_on_export: false,
                lossy_psd: false,
                export_bytes: b"EXPORTED-BYTES".to_vec(),
            },
            seen,
        )
    }
}

impl Session for Mock {
    fn open(&mut self, staged: &Path, name: &str, _: &Cancel) -> Result<Value, SessionError> {
        let bytes = std::fs::read(staged).map_err(|e| SessionError::Failed(e.to_string()))?;
        self.seen.opened.lock().unwrap().push((name.to_owned(), bytes));
        self.document = true;
        Ok(json!({"name": name, "width": 4, "height": 4}))
    }
    fn inspect(&mut self) -> Result<Value, SessionError> {
        Ok(if self.document {
            json!({"document": {"name": "mock", "layers": []}})
        } else {
            json!({"document": null})
        })
    }
    fn has_document(&self) -> bool {
        self.document
    }
    fn command_context(&mut self) -> CommandContext {
        CommandContext::default()
    }
    fn headless_commands(&self) -> &'static [&'static str] {
        &["layer.new", "edit.undo", "select.expand", "select.all", "file.check_psd"]
    }
    fn run_command(&mut self, call: &Validated) -> Result<Value, SessionError> {
        self.seen.commands.lock().unwrap().push(call.id.clone());
        if call.id == "layer.new" {
            Ok(json!({"changed": true}))
        } else {
            Err(SessionError::Refused(format!("mock refuses {}", call.id)))
        }
    }
    fn preview(&mut self, max_side: u32, _: &Cancel) -> Result<Preview, SessionError> {
        if !self.document {
            return Err(SessionError::NoDocument);
        }
        Ok(Preview {
            png: b"\x89PNG-mock".to_vec(),
            width: max_side.min(4),
            height: 3,
            document_width: 4,
            document_height: 3,
        })
    }
    fn export(
        &mut self,
        format: ExportFormat,
        accept: bool,
        out: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(f64),
    ) -> Result<ExportOutcome, SessionError> {
        self.seen.exports.fetch_add(1, Ordering::SeqCst);
        if !self.document {
            return Err(SessionError::NoDocument);
        }
        if self.hang_on_export {
            progress(0.1);
            for _ in 0..2000 {
                if cancel.is_cancelled() {
                    return Err(SessionError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            return Err(SessionError::Failed("never cancelled".into()));
        }
        if format == ExportFormat::Psd && self.lossy_psd && !accept {
            return Ok(ExportOutcome::NeedsConfirmation {
                report: json!({"asks": true}),
            });
        }
        progress(0.5);
        std::fs::write(out, &self.export_bytes).map_err(|e| SessionError::Failed(e.to_string()))?;
        progress(1.0);
        Ok(ExportOutcome::Written { report: json!({}) })
    }
    fn psd_check(&mut self, _: &Cancel) -> Result<Value, SessionError> {
        Ok(json!({"asks": false, "groups": []}))
    }
}

/// A client driving a server over an in-memory duplex stream.
pub struct Client {
    to_server: tokio::io::DuplexStream,
    lines: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    next_id: i64,
    pub notifications: Vec<Value>,
    pub server: tokio::task::JoinHandle<Result<(), aurora_mcp::ServeError>>,
}

pub fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
}

pub async fn start(mock: Mock, read: &[&Path], write: &[&Path]) -> Client {
    start_with(mock, read, write, Limits::default()).await
}

pub async fn start_with(mock: Mock, read: &[&Path], write: &[&Path], limits: Limits) -> Client {
    let read: Vec<_> = read.iter().map(|p| p.to_path_buf()).collect();
    let write: Vec<_> = write.iter().map(|p| p.to_path_buf()).collect();
    let roots = Roots::open(&read, &write).unwrap();
    let (client_to_server, server_in) = tokio::io::duplex(1 << 20);
    let (server_out, from_server) = tokio::io::duplex(1 << 20);
    let config = Config {
        roots,
        limits,
        scratch_parent: None,
    };
    let server = tokio::spawn(aurora_mcp::serve_io(
        Box::new(mock),
        config,
        server_in,
        server_out,
    ));
    let mut client = Client {
        to_server: client_to_server,
        lines: BufReader::new(from_server).lines(),
        next_id: 1,
        notifications: Vec::new(),
        server,
    };
    let init = client
        .request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        )
        .await;
    assert!(init.get("result").is_some(), "{init}");
    client.notify("notifications/initialized", json!({})).await;
    client
}

impl Client {
    pub async fn send_raw(&mut self, line: &str) {
        self.to_server.write_all(line.as_bytes()).await.unwrap();
        self.to_server.write_all(b"\n").await.unwrap();
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        self.send_raw(&line).await;
    }

    /// Sends a request and returns the reply with the same id.
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params).await;
        self.reply(id).await
    }

    pub async fn send_request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        self.send_raw(&line).await;
        id
    }

    pub async fn reply(&mut self, id: i64) -> Value {
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(30), self.lines.next_line())
                .await
                .expect("the server did not reply in time")
                .unwrap()
                .expect("the server closed the stream");
            let value: Value = serde_json::from_str(&line).expect("every output line is JSON");
            assert_eq!(value["jsonrpc"], "2.0", "{value}");
            if value.get("id") == Some(&json!(id)) {
                return value;
            }
            self.notifications.push(value);
        }
    }

    /// Reads one line (any id), for tests of parse errors.
    pub async fn next_message(&mut self) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(30), self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }

    pub async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await
    }

    /// A tool call that must succeed: the parsed JSON text result.
    pub async fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.call(tool, arguments).await;
        assert!(reply.get("error").is_none(), "{reply}");
        assert_ne!(reply["result"]["isError"], json!(true), "{reply}");
        text_json(&reply)
    }

    /// A tool call that must be a tool error; the message.
    pub async fn tool_error(&mut self, tool: &str, arguments: Value) -> String {
        let reply = self.call(tool, arguments).await;
        assert!(reply.get("error").is_none(), "expected a tool error: {reply}");
        assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
        reply["result"]["content"][0]["text"].as_str().unwrap().to_owned()
    }

    /// A call that must be a JSON-RPC error; the code.
    pub async fn rpc_error(&mut self, tool: &str, arguments: Value) -> i64 {
        let reply = self.call(tool, arguments).await;
        reply["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("expected a JSON-RPC error: {reply}"))
    }
}

pub fn text_json(reply: &Value) -> Value {
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}
