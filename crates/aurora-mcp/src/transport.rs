//! The newline-delimited JSON-RPC stream in front of `rmcp`.
//!
//! `rmcp` reads lines of any length and treats a line that is not JSON as a
//! fatal transport error. This front end makes both properties ours:
//!
//! * a request line longer than `MAX_REQUEST_BYTES` (1 MiB) ends the session
//!   (it is never forwarded, not even its valid prefix);
//! * a line that is not JSON is answered with a JSON-RPC `-32700` error
//!   (`id: null`) and the next line is handled normally;
//! * a single writer serialises those parse errors with `rmcp`'s replies,
//!   one line each, so two writers never interleave bytes on the stream.
//!
//! Nothing but protocol lines is ever written to the output.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// One line read from the client.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Text(String),
    /// The line was not UTF-8 (it is skipped with a `-32700` reply).
    NotUtf8,
}

/// Reads one request line, never holding more than `maximum` bytes.
///
/// # Errors
/// `InvalidData` when the line is longer than `maximum`.
pub(crate) async fn bounded_line<R: AsyncRead + Unpin>(
    source: &mut BufReader<R>,
    maximum: usize,
) -> std::io::Result<Option<Line>> {
    let mut bytes = Vec::new();
    loop {
        let available = source.fill_buf().await?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Ok(Some(into_line(bytes)));
        }
        let newline = available.iter().position(|&byte| byte == b'\n');
        let length = newline.map_or(available.len(), |at| at + 1);
        if length > maximum.saturating_sub(bytes.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("request exceeds {maximum} bytes"),
            ));
        }
        if let Some(chunk) = available.get(..length) {
            bytes.extend_from_slice(chunk);
        }
        source.consume(length);
        if newline.is_some() {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return Ok(Some(into_line(bytes)));
        }
    }
}

fn into_line(bytes: Vec<u8>) -> Line {
    String::from_utf8(bytes).map_or(Line::NotUtf8, Line::Text)
}

/// Requests the client sent that have no reply yet, by id (as JSON text).
type InFlight = Arc<Mutex<HashSet<String>>>;

/// Most requests allowed in flight at once.
const MAX_IN_FLIGHT: usize = 256;

fn lock(set: &InFlight) -> std::sync::MutexGuard<'_, HashSet<String>> {
    set.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A JSON-RPC error reply with the given id (`null` when unknown).
fn error_reply(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// What the reader does with one parsed line.
enum Verdict {
    Forward,
    Reply(String),
}

fn judge(value: &Value, in_flight: &InFlight) -> Verdict {
    let Some(object) = value.as_object() else {
        return Verdict::Reply(error_reply(
            &Value::Null,
            -32600,
            "invalid request: expected a JSON object (batches are not supported)",
        ));
    };
    let id = object.get("id").filter(|id| !id.is_null());
    let method = object.get("method").and_then(Value::as_str);
    if method.is_none() && !object.contains_key("result") && !object.contains_key("error") {
        return Verdict::Reply(error_reply(
            id.unwrap_or(&Value::Null),
            -32600,
            "invalid request: no method",
        ));
    }
    if method == Some("notifications/cancelled") {
        // A cancelled request gets no reply, so free its id here.
        if let Some(target) = object.get("params").and_then(|p| p.get("requestId")) {
            lock(in_flight).remove(&target.to_string());
        }
    }
    if let (Some(_), Some(id)) = (method, id) {
        let mut set = lock(in_flight);
        if set.len() >= MAX_IN_FLIGHT {
            return Verdict::Reply(error_reply(id, -32600, "too many requests in flight"));
        }
        if !set.insert(id.to_string()) {
            return Verdict::Reply(error_reply(
                id,
                -32600,
                "invalid request: that request id is already in flight",
            ));
        }
    }
    Verdict::Forward
}

/// Frees the id of a reply the writer is about to send.
fn settle(line: &str, in_flight: &InFlight) {
    if let Ok(value) = serde_json::from_str::<Value>(line)
        && value.get("method").is_none()
        && (value.get("result").is_some() || value.get("error").is_some())
        && let Some(id) = value.get("id")
    {
        lock(in_flight).remove(&id.to_string());
    }
}

/// A JSON-RPC parse error reply (`-32700`, `id: null`).
fn parse_error(message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": {"code": -32700, "message": format!("parse error: {message}")},
    })
    .to_string()
}

/// The tasks that pump the real streams into the service's.
pub(crate) struct Pumps {
    reader: tokio::task::JoinHandle<std::io::Result<()>>,
    writer: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Pumps {
    /// Starts the reader and writer tasks over `input` and `output`;
    /// returns the pumps with the two stream halves the service uses.
    pub(crate) fn start(
        input: impl AsyncRead + Unpin + Send + 'static,
        output: impl AsyncWrite + Unpin + Send + 'static,
        max_request_bytes: usize,
    ) -> (Self, tokio::io::DuplexStream, tokio::io::DuplexStream) {
        let (mut to_service, service_in) = tokio::io::duplex(1 << 20);
        let (service_out, from_service) = tokio::io::duplex(1 << 20);
        let (errors, mut error_rx) = tokio::sync::mpsc::channel::<String>(16);
        let in_flight: InFlight = Arc::new(Mutex::new(HashSet::new()));
        let reader_set = Arc::clone(&in_flight);
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(input);
            while let Some(read) = bounded_line(&mut lines, max_request_bytes).await? {
                let line = match read {
                    Line::Text(line) => line,
                    Line::NotUtf8 => {
                        if errors
                            .send(parse_error("the line is not valid UTF-8"))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => match judge(&value, &reader_set) {
                        Verdict::Forward => {
                            to_service.write_all(line.as_bytes()).await?;
                            to_service.write_all(b"\n").await?;
                        }
                        Verdict::Reply(reply) => {
                            if errors.send(reply).await.is_err() {
                                break;
                            }
                        }
                    },
                    Err(error) => {
                        if errors.send(parse_error(&error.to_string())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            to_service.shutdown().await
        });
        let writer = tokio::spawn(async move {
            let mut output = output;
            let mut replies = BufReader::new(from_service).lines();
            loop {
                let (line, from_service) = tokio::select! {
                    biased;
                    Some(error) = error_rx.recv() => (error, false),
                    reply = replies.next_line() => match reply? {
                        Some(line) => (line, true),
                        None => break,
                    },
                };
                if from_service {
                    settle(&line, &in_flight);
                }
                output.write_all(line.as_bytes()).await?;
                output.write_all(b"\n").await?;
                output.flush().await?;
            }
            Ok::<_, std::io::Error>(())
        });
        (Self { reader, writer }, service_in, service_out)
    }

    /// Stops the pumps; the first transport error (an oversized line, a
    /// broken pipe) is returned.
    pub(crate) async fn finish(self) -> std::io::Result<()> {
        let Self { reader, writer } = self;
        reader.abort();
        match reader.await {
            Ok(result) => result?,
            Err(error) if !error.is_cancelled() => {
                return Err(std::io::Error::other(error.to_string()));
            }
            Err(_) => {}
        }
        writer
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?
    }
}
