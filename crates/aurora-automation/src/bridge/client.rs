//! The controller side: connect to the app, authenticate, make calls.

use std::io::BufReader;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::discovery::Discovery;
use super::frame::{self, Conn, Line};
use super::protocol::{
    CallError, Hello, HelloReply, Id, MAX_HELLO_BYTES, MAX_REPLY_BYTES, Reply, Request,
};

/// Why a call did not return a result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// Nothing was sent: the connection could not be made or refused us.
    #[error("couldn't connect to the Aurora app: {0}")]
    Connect(String),
    /// The app answered with an error.
    #[error("{0}")]
    App(CallError),
    /// The request may or may not have been carried out.
    #[error("the connection to the Aurora app failed ({0}); the operation may have completed")]
    Transport(String),
    /// No reply in time; the operation may still be running.
    #[error("no reply from the Aurora app in {0} s; the operation may still complete")]
    Timeout(u64),
}

/// An authenticated connection.
#[derive(Debug)]
pub struct BridgeClient {
    stream: Conn,
    reader: BufReader<Conn>,
    next_id: u64,
    pub app_version: String,
}

impl BridgeClient {
    /// Connects to the app the discovery record describes and authenticates.
    /// Only the loopback address is ever dialled, whatever the file says.
    ///
    /// # Errors
    /// [`ClientError::Connect`] with the reason (including the app's
    /// refusal: wrong token, another controller).
    pub fn connect(info: &Discovery) -> Result<Self, ClientError> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), info.port);
        Self::connect_to(addr, &info.token, Duration::from_secs(5))
    }

    /// [`BridgeClient::connect`] to an explicit loopback address.
    ///
    /// # Errors
    /// As [`BridgeClient::connect`]; a non-loopback address is refused.
    pub fn connect_to(
        addr: SocketAddr,
        token: &str,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        if !addr.ip().is_loopback() {
            return Err(ClientError::Connect(
                "only loopback addresses are dialled".into(),
            ));
        }
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let stream = Conn::new(stream);
        let mut writer = stream.clone();
        let mut reader = BufReader::new(stream.clone());
        let hello = serde_json::to_string(&Hello::new(token))
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        frame::write_line(&mut writer, &hello).map_err(|e| ClientError::Connect(e.to_string()))?;
        let line =
            match frame::read_line_by(&mut reader, MAX_HELLO_BYTES * 4, Instant::now() + timeout) {
                Ok(Line::Data(line)) => line,
                Ok(_) => return Err(ClientError::Connect("the app closed the connection".into())),
                Err(e) => return Err(ClientError::Connect(e.to_string())),
            };
        let reply: HelloReply = serde_json::from_slice(&line)
            .map_err(|e| ClientError::Connect(format!("unexpected handshake reply: {e}")))?;
        if !reply.ok {
            return Err(ClientError::Connect(
                reply.error.unwrap_or_else(|| "refused".to_owned()),
            ));
        }
        Ok(Self {
            stream,
            reader,
            next_id: 1,
            app_version: reply.app_version.unwrap_or_default(),
        })
    }

    /// Makes one call and waits at most `timeout` for its reply.
    ///
    /// # Errors
    /// See [`ClientError`].
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id: Id::Number(id),
            method: method.to_owned(),
            params,
        };
        let text =
            serde_json::to_string(&request).map_err(|e| ClientError::Transport(e.to_string()))?;
        frame::write_line(&mut self.stream, &text)
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        let deadline = Instant::now() + timeout;
        loop {
            let line = match frame::read_line_by(&mut self.reader, MAX_REPLY_BYTES, deadline) {
                Ok(Line::Data(line)) => line,
                Ok(Line::Eof) => {
                    return Err(ClientError::Transport(
                        "the app closed the connection".into(),
                    ));
                }
                Ok(Line::TooLong) => {
                    return Err(ClientError::Transport("a reply was too long".into()));
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    return Err(ClientError::Timeout(timeout.as_secs()));
                }
                Err(e) => return Err(ClientError::Transport(e.to_string())),
            };
            let Ok(reply) = serde_json::from_slice::<Reply>(&line) else {
                continue;
            };
            match reply.id {
                Some(Id::Number(n)) if n == id => {}
                None if !reply.ok => {} // a connection-level error
                _ => continue,          // a stale line from an earlier call
            }
            return if reply.ok {
                Ok(reply.result.unwrap_or(Value::Null))
            } else {
                Err(ClientError::App(reply.error.unwrap_or_else(|| {
                    CallError::new("failed", "the app reported an error")
                })))
            };
        }
    }
}
