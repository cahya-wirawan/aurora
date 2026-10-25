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

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Reads one request line, never holding more than `maximum` bytes.
///
/// # Errors
/// `InvalidData` when the line is longer than `maximum` or not UTF-8.
pub(crate) async fn bounded_line<R: AsyncRead + Unpin>(
    source: &mut BufReader<R>,
    maximum: usize,
) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let available = source.fill_buf().await?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return into_text(bytes).map(Some);
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
            return into_text(bytes).map(Some);
        }
    }
}

fn into_text(bytes: Vec<u8>) -> std::io::Result<String> {
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
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
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(input);
            while let Some(line) = bounded_line(&mut lines, max_request_bytes).await? {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(_) => {
                        to_service.write_all(line.as_bytes()).await?;
                        to_service.write_all(b"\n").await?;
                    }
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
                let line = tokio::select! {
                    biased;
                    Some(error) = error_rx.recv() => error,
                    reply = replies.next_line() => match reply? {
                        Some(line) => line,
                        None => break,
                    },
                };
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
