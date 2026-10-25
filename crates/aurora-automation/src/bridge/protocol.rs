//! The wire messages. One JSON object per line.
//!
//! ```text
//! client -> {"hello":"aurora-bridge","version":1,"token":"<64 hex>"}
//! server -> {"ok":true,"protocol":1,"app_version":"0.295.0","pid":1234}
//! client -> {"id":1,"method":"ui.inspect","params":{}}
//! server -> {"id":1,"ok":true,"result":{...}}
//!        or {"id":1,"ok":false,"error":{"kind":"modal_dialog","message":"..."}}
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version both sides must speak.
pub const PROTOCOL_VERSION: u32 = 1;
/// The `hello` value of the handshake.
pub const HELLO: &str = "aurora-bridge";
/// The longest handshake line accepted, in bytes (a token handshake is
/// about 100).
pub const MAX_HELLO_BYTES: usize = 512;
/// The longest request line accepted from the controller (1 MiB, like the
/// MCP transport's).
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// The longest reply line the proxy accepts from the app (a screenshot or
/// preview PNG as base64 fits).
pub const MAX_REPLY_BYTES: usize = 16 << 20;
/// The longest method name accepted.
pub const MAX_METHOD_BYTES: usize = 64;
/// The longest string id accepted.
pub const MAX_ID_BYTES: usize = 64;

/// The first line a client sends. Unknown fields are refused: the handshake
/// is exactly this and nothing else.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub hello: String,
    pub version: u32,
    pub token: String,
}

impl Hello {
    #[must_use]
    pub fn new(token: &str) -> Self {
        Self {
            hello: HELLO.to_owned(),
            version: PROTOCOL_VERSION,
            token: token.to_owned(),
        }
    }
}

/// The server's answer to the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A request id: a number or a short string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(u64),
    Text(String),
}

/// One request.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: Id,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    /// Whether the request's own fields are within bounds.
    ///
    /// # Errors
    /// A sentence saying what is too long or empty.
    pub fn check(&self) -> Result<(), String> {
        if self.method.is_empty() || self.method.len() > MAX_METHOD_BYTES {
            return Err(format!(
                "`method` must be 1 to {MAX_METHOD_BYTES} bytes, not {}",
                self.method.len()
            ));
        }
        if !self
            .method
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        {
            return Err("`method` may hold only letters, digits, `.` and `_`".to_owned());
        }
        if let Id::Text(text) = &self.id
            && text.len() > MAX_ID_BYTES
        {
            return Err(format!("a string `id` is at most {MAX_ID_BYTES} bytes"));
        }
        if !(self.params.is_object() || self.params.is_null()) {
            return Err("`params` must be an object".to_owned());
        }
        Ok(())
    }
}

/// Why a call failed, as the app tells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallError {
    /// A short machine-readable kind: `bad_request`, `unknown_method`,
    /// `modal_dialog`, `timeout`, `rate_limited`, `refused`, `failed`, ...
    pub kind: String,
    pub message: String,
}

impl CallError {
    #[must_use]
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// One reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub id: Option<Id>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CallError>,
}

impl Reply {
    #[must_use]
    pub fn success(id: Option<Id>, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn failure(id: Option<Id>, error: CallError) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}
