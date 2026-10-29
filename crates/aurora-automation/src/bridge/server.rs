//! The listener the desktop app embeds.
//!
//! * **Loopback only.** It binds `127.0.0.1` (an ephemeral port by default),
//!   refuses to be configured with any other address, and drops a peer whose
//!   address is not loopback.
//! * **The token is the first line.** A connection must send exactly the
//!   [`Hello`] handshake within the authentication timeout. Anything else
//!   (garbage, a different JSON shape, a slow trickle, an oversized line)
//!   closes it; an HTTP request (what a web page can send) is dropped
//!   without an answer and without counting as a failed attempt. The token
//!   is compared in constant time ([`token::verify`]).
//! * **No lockout.** A failed handshake costs only its own connection: it
//!   gets one attempt, is answered after a short fixed delay
//!   ([`ServerConfig::fail_delay`]) and is closed. Nothing a stream of wrong
//!   tokens does can refuse the connection that carries the right one (a
//!   lockout would be a denial of service against the real controller). The
//!   256-bit token, not a throttle, is what makes guessing pointless.
//! * **One controller.** A second authenticated connection is refused while
//!   one is active. At most [`ServerConfig::max_unauthenticated`] handshakes
//!   are in progress at once; when a new connection arrives and they are all
//!   taken, the connection that has already failed, else the oldest one still
//!   waiting for its handshake, is dropped to make room. Silent connections
//!   therefore cannot hold the slots, and the handshake must arrive within
//!   [`ServerConfig::auth_timeout`].
//! * **Bounded input.** A request line is at most
//!   [`super::protocol::MAX_REQUEST_BYTES`]; a longer one closes the connection
//!   and is never acted on. Requests are rate-limited.
//! * Each connection is served by its own thread, one request at a time;
//!   the [`Dispatcher`] decides where a request runs (the app marshals it to
//!   its UI thread and waits with a timeout; the UI thread never waits).

use std::collections::VecDeque;
use std::io::{self, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::frame::{self, Conn, Line};
use super::limits::RateLimiter;
use super::protocol::{
    CallError, HELLO, Hello, HelloReply, MAX_HELLO_BYTES, MAX_REQUEST_BYTES, PROTOCOL_VERSION,
    Reply, Request,
};
use super::token;

/// Default for [`ServerConfig::max_unauthenticated`].
pub const MAX_UNAUTHENTICATED: usize = 8;
/// Consecutive malformed requests after which a controller is dropped.
const MAX_BAD_STREAK: u32 = 8;

/// Where a request is carried out.
pub trait Dispatcher: Send + Sync + 'static {
    /// Handles one request on the connection's own thread. It may block
    /// (the app waits for its UI thread with a timeout); it must not panic.
    ///
    /// # Errors
    /// The failure the controller is told.
    fn dispatch(&self, request: Request) -> Result<Value, CallError>;

    /// A controller authenticated.
    fn controller_connected(&self) {}

    /// The controller is gone (closed, kicked, timed out).
    fn controller_disconnected(&self) {}
}

/// Listener settings. The defaults are the production values.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Must be a loopback address (default `127.0.0.1:0`).
    pub bind: SocketAddr,
    /// The whole handshake line must arrive within this.
    pub auth_timeout: Duration,
    /// Handshakes in progress at once (more than this evicts the oldest).
    pub max_unauthenticated: usize,
    /// How long a failed handshake is held before its refusal is sent.
    pub fail_delay: Duration,
    /// A controller that sends nothing for this long is dropped (the proxy
    /// reconnects on its next call).
    pub idle_timeout: Duration,
    /// Request rate: a burst and a sustained rate.
    pub rate_burst: u32,
    pub rate_per_second: f64,
    /// Longest request line.
    pub max_request_bytes: usize,
    /// The app version, told to the controller.
    pub app_version: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            auth_timeout: Duration::from_secs(2),
            max_unauthenticated: MAX_UNAUTHENTICATED,
            fail_delay: Duration::from_millis(250),
            idle_timeout: Duration::from_mins(5),
            rate_burst: 32,
            rate_per_second: 20.0,
            max_request_bytes: MAX_REQUEST_BYTES,
            app_version: String::new(),
        }
    }
}

/// A connection that has not authenticated yet.
struct Waiting {
    id: u64,
    conn: Conn,
    /// Set once the connection has presented a wrong token: it is doomed, so
    /// it is the first to be dropped when room is needed.
    failed: Arc<AtomicBool>,
}

struct Shared {
    /// The current token (replaced by [`BridgeServer::set_token`]).
    token: Mutex<String>,
    config: ServerConfig,
    dispatcher: Arc<dyn Dispatcher>,
    /// Connections still to authenticate, oldest first.
    waiting: Mutex<VecDeque<Waiting>>,
    next_waiting: AtomicU64,
    /// The active controller's stream (a clone, to close it from outside).
    controller: Mutex<Option<Conn>>,
    stop: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running listener.
pub struct BridgeServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for BridgeServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl BridgeServer {
    /// Starts listening.
    ///
    /// # Errors
    /// `bind` is not a loopback address, the token is malformed, or the
    /// socket could not be bound.
    pub fn start(
        config: ServerConfig,
        token_text: String,
        dispatcher: Arc<dyn Dispatcher>,
    ) -> io::Result<Self> {
        if !config.bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the bridge only listens on loopback, not {}",
                    config.bind.ip()
                ),
            ));
        }
        if !token::well_formed(&token_text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the bridge token must be 64 hexadecimal characters",
            ));
        }
        let listener = TcpListener::bind(config.bind)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            token: Mutex::new(token_text),
            config,
            dispatcher,
            waiting: Mutex::new(VecDeque::new()),
            next_waiting: AtomicU64::new(1),
            controller: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let accept_shared = Arc::clone(&shared);
        let accept = std::thread::Builder::new()
            .name("aurora-bridge-accept".to_owned())
            .spawn(move || accept_loop(&listener, &accept_shared))?;
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
        })
    }

    /// The address it listens on.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Replaces the token: connections that present the old one are refused
    /// from now on. The controller already connected stays connected.
    ///
    /// # Errors
    /// The new token is malformed (nothing changes).
    pub fn set_token(&self, token_text: &str) -> io::Result<()> {
        if !token::well_formed(token_text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the bridge token must be 64 hexadecimal characters",
            ));
        }
        *lock(&self.shared.token) = token_text.to_owned();
        Ok(())
    }

    /// Whether a controller is connected.
    #[must_use]
    pub fn controller_connected(&self) -> bool {
        lock(&self.shared.controller).is_some()
    }

    /// Closes the connected controller's socket (its thread then ends).
    pub fn disconnect_controller(&self) {
        if let Some(stream) = lock(&self.shared.controller).as_ref() {
            stream.close();
        }
    }

    /// Stops listening and closes the controller. After this the token is
    /// useless; new connections are refused.
    pub fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.disconnect_controller();
        // Wake the blocked accept so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for incoming in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = incoming else { continue };
        let loopback = stream.peer_addr().is_ok_and(|peer| peer.ip().is_loopback());
        if !loopback {
            tracing::warn!("bridge: dropped a connection from a non-loopback address");
            continue;
        }
        let conn = Conn::new(stream);
        let failed = Arc::new(AtomicBool::new(false));
        let id = shared.next_waiting.fetch_add(1, Ordering::Relaxed);
        {
            // Make room by dropping a connection that has already failed,
            // else the oldest one still waiting; never refuse the new one.
            let mut waiting = lock(&shared.waiting);
            let capacity = shared.config.max_unauthenticated.max(1);
            while waiting.len() >= capacity {
                let victim = waiting
                    .iter()
                    .position(|w| w.failed.load(Ordering::SeqCst))
                    .unwrap_or(0);
                if let Some(evicted) = waiting.remove(victim) {
                    tracing::debug!("bridge: dropped a connection waiting to authenticate");
                    evicted.conn.close();
                }
            }
            waiting.push_back(Waiting {
                id,
                conn: conn.clone(),
                failed: Arc::clone(&failed),
            });
        }
        let conn_shared = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            .name("aurora-bridge-conn".to_owned())
            .spawn(move || {
                serve_connection(&conn, &conn_shared, id, &failed);
            });
        if spawned.is_err() {
            forget(shared, id);
        }
    }
}

/// Takes a connection off the waiting list (it authenticated or ended).
fn forget(shared: &Shared, id: u64) {
    lock(&shared.waiting).retain(|w| w.id != id);
}

/// Ends a connection after a reply without losing the reply.
///
/// Closing a socket that still holds unread data from the peer makes Windows
/// send a reset, and the reset discards what the peer had not read yet: the
/// client saw "connection aborted" (os error 10053) instead of the refusal.
/// So the write side is closed first (the reply is delivered, then a FIN) and
/// whatever the peer already sent is read and thrown away, for at most
/// `patience` and 64 KiB, before the socket is dropped.
fn close_after_reply(stream: &TcpStream, patience: Duration) {
    let _ = stream.shutdown(Shutdown::Write);
    let deadline = Instant::now() + patience;
    let mut seen = 0_usize;
    let mut buf = [0_u8; 4096];
    let mut reader = stream;
    while seen < 64 * 1024 {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let left = deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(1));
        if stream.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => seen += n,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// A request line that is HTTP: what a web page can send. Browsers cannot
/// send anything else to a raw port, so this is the cross-origin case.
fn looks_like_http(line: &[u8]) -> bool {
    const METHODS: [&[u8]; 9] = [
        b"GET ",
        b"POST ",
        b"PUT ",
        b"HEAD ",
        b"OPTIONS ",
        b"DELETE ",
        b"PATCH ",
        b"CONNECT ",
        b"TRACE ",
    ];
    METHODS.iter().any(|m| line.starts_with(m))
        || line
            .windows(8)
            .any(|w| w == b" HTTP/1." || w == b" HTTP/2.")
}

/// Takes the connection off the waiting list on every exit path.
struct Unauthenticated<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for Unauthenticated<'_> {
    fn drop(&mut self) {
        forget(self.shared, self.id);
    }
}

/// Holds the single controller slot; releasing it tells the dispatcher.
struct Slot<'a> {
    shared: &'a Shared,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *lock(&self.shared.controller) = None;
        self.shared.dispatcher.controller_disconnected();
    }
}

fn serve_connection(conn: &Conn, shared: &Arc<Shared>, id: u64, failed: &AtomicBool) {
    let counted = Unauthenticated { shared, id };
    let stream = conn.socket();
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut writer = conn.clone();
    let mut reader = BufReader::new(conn.clone());

    // ---- handshake: exactly one bounded line, within the deadline ----
    let deadline = Instant::now() + shared.config.auth_timeout;
    let line = match frame::read_line_by(&mut reader, MAX_HELLO_BYTES, deadline) {
        Ok(Line::Data(line)) => line,
        Ok(Line::TooLong) => {
            // Not a handshake at all: refused, not counted.
            reject(&mut writer, "the handshake is too long");
            return;
        }
        // Closed without a line, or too slow: nothing to answer, and a
        // connection that sends nothing is not an authentication attempt
        // worth counting (a port scanner, a liveness probe).
        Ok(Line::Eof) | Err(_) => {
            // Close the socket explicitly: a silent client must not be left
            // waiting for the handles to be released.
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };
    if looks_like_http(&line) {
        tracing::debug!("bridge: dropped an HTTP request on the control socket");
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }
    // Only a WELL-FORMED hello with a wrong token counts as a failed
    // attempt. Binary junk, a TLS ClientHello, invalid UTF-8, JSON of
    // another shape: refused, never counted, so something that cannot even
    // speak the protocol cannot lock the real controller out.
    let Ok(hello) = serde_json::from_slice::<Hello>(&line) else {
        reject(&mut writer, "bad handshake");
        return;
    };
    if hello.hello != HELLO {
        reject(&mut writer, "bad handshake");
        return;
    }
    let expected = lock(&shared.token).clone();
    if !token::verify(&hello.token, &expected) {
        failed.store(true, Ordering::SeqCst);
        fail_auth(shared, &mut writer, "authentication failed");
        return;
    }
    if hello.version != PROTOCOL_VERSION {
        reply_hello_error(
            &mut writer,
            &format!(
                "unsupported protocol version {}; this app speaks {PROTOCOL_VERSION}",
                hello.version
            ),
        );
        return;
    }

    // ---- the single controller slot ----
    {
        let mut slot = lock(&shared.controller);
        if slot.is_some() {
            drop(slot);
            reply_hello_error(&mut writer, "another controller is already connected");
            return;
        }
        *slot = Some(conn.clone());
    }
    drop(counted);
    let _slot = Slot { shared };
    let ok = HelloReply {
        ok: true,
        protocol: Some(PROTOCOL_VERSION),
        app_version: Some(shared.config.app_version.clone()),
        pid: Some(std::process::id()),
        error: None,
    };
    let Ok(text) = serde_json::to_string(&ok) else {
        return;
    };
    if frame::write_line(&mut writer, &text).is_err() {
        return;
    }
    tracing::info!("bridge: a controller connected");
    shared.dispatcher.controller_connected();
    request_loop(&mut reader, &mut writer, shared);
    tracing::info!("bridge: the controller disconnected");
    let _ = stream.shutdown(Shutdown::Both);
}

fn reply_hello_error(writer: &mut Conn, message: &str) {
    let reply = HelloReply {
        ok: false,
        protocol: None,
        app_version: None,
        pid: None,
        error: Some(message.to_owned()),
    };
    if let Ok(text) = serde_json::to_string(&reply) {
        let _ = frame::write_line(writer, &text);
    }
    close_after_reply(writer.socket(), Duration::from_millis(100));
}

/// Refuses a connection that did not present a well-formed handshake.
fn reject(writer: &mut Conn, message: &str) {
    tracing::debug!("bridge: a connection did not send a handshake");
    reply_hello_error(writer, message);
}

/// A well-formed handshake with the wrong token: the refusal is sent after
/// a short fixed delay (cut short if the connection is dropped meanwhile)
/// and the connection is closed. Nothing is remembered: the next connection
/// starts clean, so failures can never keep the right token out.
fn fail_auth(shared: &Shared, writer: &mut Conn, message: &str) {
    tracing::warn!("bridge: a connection failed the handshake");
    let until = Instant::now() + shared.config.fail_delay;
    while Instant::now() < until && !writer.is_closed() && !shared.stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    if writer.is_closed() {
        return;
    }
    reply_hello_error(writer, message);
}

fn request_loop(reader: &mut BufReader<Conn>, writer: &mut Conn, shared: &Arc<Shared>) {
    let mut limiter = RateLimiter::new(shared.config.rate_burst, shared.config.rate_per_second);
    let mut bad_streak = 0_u32;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let deadline = Instant::now() + shared.config.idle_timeout;
        let line = match frame::read_line_by(reader, shared.config.max_request_bytes, deadline) {
            Ok(Line::Data(line)) => line,
            Ok(Line::TooLong) => {
                let reply = Reply::failure(
                    None,
                    CallError::new(
                        "request_too_long",
                        format!(
                            "a request line is at most {} bytes",
                            shared.config.max_request_bytes
                        ),
                    ),
                );
                let _ = send(writer, &reply);
                return;
            }
            Ok(Line::Eof) | Err(_) => return,
        };
        if reader.get_ref().is_closed() {
            return;
        }
        let request: Request = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(error) => {
                let reply = Reply::failure(
                    None,
                    CallError::new("bad_request", format!("not a request: {error}")),
                );
                bad_streak += 1;
                if send(writer, &reply).is_err() || bad_streak >= MAX_BAD_STREAK {
                    return;
                }
                continue;
            }
        };
        let id = request.id.clone();
        if let Err(message) = request.check() {
            bad_streak += 1;
            let reply = Reply::failure(Some(id), CallError::new("bad_request", message));
            if send(writer, &reply).is_err() || bad_streak >= MAX_BAD_STREAK {
                return;
            }
            continue;
        }
        bad_streak = 0;
        if !limiter.allow(Instant::now()) {
            let reply = Reply::failure(
                Some(id),
                CallError::new("rate_limited", "too many requests; slow down"),
            );
            if send(writer, &reply).is_err() {
                return;
            }
            continue;
        }
        let reply = match shared.dispatcher.dispatch(request) {
            Ok(result) => Reply::success(Some(id), result),
            Err(error) => Reply::failure(Some(id), error),
        };
        if send(writer, &reply).is_err() {
            return;
        }
    }
}

fn send(writer: &mut Conn, reply: &Reply) -> io::Result<()> {
    let mut text = serde_json::to_string(reply).map_err(|e| io::Error::other(e.to_string()))?;
    if text.len() > super::protocol::MAX_REPLY_BYTES {
        let replacement = Reply::failure(
            reply.id.clone(),
            CallError::new(
                "reply_too_large",
                "the reply is larger than the bridge allows; the operation may have completed",
            ),
        );
        text = serde_json::to_string(&replacement).map_err(|e| io::Error::other(e.to_string()))?;
    }
    frame::write_line(writer, &text)
}
