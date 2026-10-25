//! The bridge listener against real sockets: handshake, authentication,
//! lockout, limits, single controller, loopback-only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aurora_automation::bridge::client::{BridgeClient, ClientError};
use aurora_automation::bridge::protocol::{CallError, Request};
use aurora_automation::bridge::server::{BridgeServer, Dispatcher, ServerConfig};
use aurora_automation::bridge::token;
use serde_json::{Value, json};

#[derive(Default)]
struct Echo {
    calls: AtomicUsize,
    connects: AtomicUsize,
    disconnects: AtomicUsize,
}

impl Dispatcher for Echo {
    fn dispatch(&self, request: Request) -> Result<Value, CallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match request.method.as_str() {
            "echo" => Ok(request.params),
            "fail" => Err(CallError::new("refused", "no")),
            _ => Err(CallError::new("unknown_method", request.method)),
        }
    }
    fn controller_connected(&self) {
        self.connects.fetch_add(1, Ordering::SeqCst);
    }
    fn controller_disconnected(&self) {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
    }
}

fn start(config: ServerConfig) -> (BridgeServer, String, Arc<Echo>) {
    let token = token::generate().unwrap();
    let echo = Arc::new(Echo::default());
    let server = BridgeServer::start(config, token.clone(), echo.clone()).unwrap();
    (server, token, echo)
}

fn fast() -> ServerConfig {
    ServerConfig {
        auth_timeout: Duration::from_millis(400),
        idle_timeout: Duration::from_secs(5),
        lockout_base: Duration::from_millis(300),
        lockout_max: Duration::from_millis(600),
        ..ServerConfig::default()
    }
}

fn raw(server: &BridgeServer) -> TcpStream {
    let s = TcpStream::connect(server.local_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s
}

fn read_all_line(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    line
}

fn hello(token: &str) -> String {
    format!("{{\"hello\":\"aurora-bridge\",\"version\":1,\"token\":\"{token}\"}}\n")
}

#[test]
fn it_binds_loopback_ipv4_only() {
    let (server, ..) = start(ServerConfig::default());
    assert_eq!(server.local_addr().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(server.local_addr().port(), 0);
}

#[test]
fn a_non_loopback_bind_is_refused() {
    for ip in [Ipv4Addr::UNSPECIFIED, Ipv4Addr::new(192, 168, 1, 5)] {
        let config = ServerConfig {
            bind: SocketAddr::new(IpAddr::V4(ip), 0),
            ..ServerConfig::default()
        };
        let result = BridgeServer::start(
            config,
            token::generate().unwrap(),
            Arc::new(Echo::default()),
        );
        assert!(result.is_err(), "{ip} must be refused");
    }
    let v6 = ServerConfig {
        bind: SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
        ..ServerConfig::default()
    };
    assert!(
        BridgeServer::start(v6, token::generate().unwrap(), Arc::new(Echo::default())).is_err()
    );
}

#[test]
fn a_malformed_token_cannot_start_a_server() {
    let result = BridgeServer::start(
        ServerConfig::default(),
        "short".into(),
        Arc::new(Echo::default()),
    );
    assert!(result.is_err());
}

#[test]
fn the_right_token_authenticates_and_calls_work() {
    let (server, token, echo) = start(fast());
    let mut client =
        BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap();
    let out = client
        .call("echo", json!({"a": 1}), Duration::from_secs(3))
        .unwrap();
    assert_eq!(out, json!({"a": 1}));
    let err = client
        .call("fail", json!({}), Duration::from_secs(3))
        .unwrap_err();
    assert!(
        matches!(err, ClientError::App(ref e) if e.kind == "refused"),
        "{err:?}"
    );
    assert_eq!(echo.calls.load(Ordering::SeqCst), 2);
    assert!(server.controller_connected());
}

#[test]
fn authentication_goes_through_the_constant_time_verify() {
    let (server, token, _) = start(fast());
    let before = token::verifications();
    drop(BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap());
    assert!(
        token::verifications() > before,
        "the listener must call token::verify"
    );
}

#[test]
fn a_wrong_token_is_refused_and_nothing_is_dispatched() {
    let (server, _, echo) = start(fast());
    let wrong = "0".repeat(64);
    let err =
        BridgeClient::connect_to(server.local_addr(), &wrong, Duration::from_secs(3)).unwrap_err();
    assert!(err.to_string().contains("authentication failed"), "{err}");
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
    assert!(!server.controller_connected());
}

#[test]
fn a_request_without_a_handshake_is_refused_and_never_dispatched() {
    let (server, _, echo) = start(fast());
    let mut s = raw(&server);
    s.write_all(b"{\"id\":1,\"method\":\"echo\",\"params\":{}}\n")
        .unwrap();
    let line = read_all_line(&mut s);
    assert!(line.contains("\"ok\":false"), "{line}");
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn garbage_and_wrong_shapes_are_refused() {
    let (server, token, echo) = start(ServerConfig {
        free_failures: 100,
        ..fast()
    });
    for bad in [
        "garbage\n".to_owned(),
        "[]\n".to_owned(),
        "{}\n".to_owned(),
        format!(
            "{{\"hello\":\"aurora-bridge\",\"version\":1,\"token\":\"{token}\",\"extra\":1}}\n"
        ),
        format!("{{\"hello\":\"other\",\"version\":1,\"token\":\"{token}\"}}\n"),
        "\u{0}\u{1}\n".to_owned(),
    ] {
        let mut s = raw(&server);
        s.write_all(bad.as_bytes()).unwrap();
        let line = read_all_line(&mut s);
        assert!(line.contains("\"ok\":false"), "{bad:?} -> {line}");
    }
    assert!(!server.controller_connected());
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unsupported_protocol_version_is_told_apart_after_the_token_checks_out() {
    let (server, token, _) = start(fast());
    let mut s = raw(&server);
    s.write_all(
        format!("{{\"hello\":\"aurora-bridge\",\"version\":99,\"token\":\"{token}\"}}\n")
            .as_bytes(),
    )
    .unwrap();
    assert!(read_all_line(&mut s).contains("unsupported protocol version"));
}

#[test]
fn a_slow_client_is_cut_off_at_the_deadline_even_if_it_drips_bytes() {
    let (server, token, _) = start(fast());
    let mut s = raw(&server);
    let start_time = Instant::now();
    let line = hello(&token);
    // Drip one byte at a time, slower in total than the 400 ms deadline.
    let mut cut = false;
    for byte in line.as_bytes() {
        if s.write_all(&[*byte]).is_err() {
            cut = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
        if start_time.elapsed() > Duration::from_secs(3) {
            break;
        }
    }
    s.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    assert!(
        !String::from_utf8_lossy(&rest).contains("\"ok\":true"),
        "must not authenticate"
    );
    assert!(cut || start_time.elapsed() < Duration::from_secs(4));
    assert!(!server.controller_connected());
}

#[test]
fn a_silent_connection_is_dropped_by_the_auth_timeout() {
    let (server, _, _) = start(fast());
    let mut s = raw(&server);
    let t = Instant::now();
    let mut buf = [0_u8; 8];
    let n = s.read(&mut buf).unwrap_or(0);
    assert_eq!(n, 0, "closed without an answer");
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn an_oversized_handshake_line_is_refused() {
    let (server, _, _) = start(fast());
    let mut s = raw(&server);
    let _ = s.write_all(&vec![b'a'; 5000]);
    let _ = s.write_all(b"\n");
    let line = read_all_line(&mut s);
    assert!(line.contains("too long") || line.is_empty(), "{line}");
    assert!(!server.controller_connected());
}

#[test]
fn an_http_request_is_dropped_without_an_answer_and_does_not_count_as_a_failure() {
    let (server, token, _) = start(ServerConfig {
        free_failures: 1,
        ..fast()
    });
    for _ in 0..6 {
        let mut s = raw(&server);
        s.write_all(b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf);
        assert!(
            buf.is_empty(),
            "no HTTP answer: {:?}",
            String::from_utf8_lossy(&buf)
        );
    }
    // Six probes did not lock the real controller out.
    BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap();
}

#[test]
fn repeated_failures_lock_connections_out_for_a_while() {
    let (server, token, _) = start(ServerConfig {
        free_failures: 2,
        ..fast()
    });
    let wrong = "1".repeat(64);
    for _ in 0..3 {
        let _ = BridgeClient::connect_to(server.local_addr(), &wrong, Duration::from_secs(3));
    }
    // Locked out: even the right token is refused now.
    let err =
        BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap_err();
    assert!(
        err.to_string().contains("too many failed attempts"),
        "{err}"
    );
    std::thread::sleep(Duration::from_millis(1100));
    BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap();
}

#[test]
fn a_second_controller_is_refused_while_one_is_active() {
    let (server, token, _) = start(fast());
    let mut first =
        BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap();
    let err =
        BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap_err();
    assert!(err.to_string().contains("another controller"), "{err}");
    // The first is unharmed.
    first
        .call("echo", json!({}), Duration::from_secs(3))
        .unwrap();
    drop(first);
    // After it leaves, a new one is accepted.
    let mut second = None;
    for _ in 0..40 {
        if let Ok(c) = BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3))
        {
            second = Some(c);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(second.is_some());
}

#[test]
fn an_oversized_request_closes_the_connection_and_is_not_dispatched() {
    let (server, token, echo) = start(ServerConfig {
        max_request_bytes: 2048,
        ..fast()
    });
    let mut s = raw(&server);
    s.write_all(hello(&token).as_bytes()).unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("\"ok\":true"));
    let big = format!(
        "{{\"id\":1,\"method\":\"echo\",\"params\":{{\"x\":\"{}\"}}}}\n",
        "a".repeat(4000)
    );
    s.write_all(big.as_bytes()).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("request_too_long"), "{line}");
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap_or(0), 0, "closed");
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn malformed_requests_get_errors_and_a_bad_streak_closes() {
    let (server, token, _) = start(fast());
    let mut s = raw(&server);
    s.write_all(hello(&token).as_bytes()).unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    for _ in 0..8 {
        s.write_all(b"not json\n").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("bad_request"), "{line}");
    }
    line.clear();
    assert_eq!(
        reader.read_line(&mut line).unwrap_or(0),
        0,
        "closed after the streak"
    );
}

#[test]
fn requests_are_rate_limited() {
    let (server, token, _) = start(ServerConfig {
        rate_burst: 3,
        rate_per_second: 0.1,
        ..fast()
    });
    let mut client =
        BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap();
    let mut limited = 0;
    for _ in 0..8 {
        if let Err(ClientError::App(e)) = client.call("echo", json!({}), Duration::from_secs(3))
            && e.kind == "rate_limited"
        {
            limited += 1;
        }
    }
    assert!(limited >= 4, "limited {limited}");
}

#[test]
fn disconnecting_closes_the_controller_and_shutdown_revokes_the_door() {
    let (mut server, token, echo) = start(fast());
    let mut client =
        BridgeClient::connect_to(server.local_addr(), &token, Duration::from_secs(3)).unwrap();
    server.disconnect_controller();
    let err = client
        .call("echo", json!({}), Duration::from_secs(2))
        .unwrap_err();
    assert!(
        matches!(err, ClientError::Transport(_) | ClientError::Connect(_)),
        "{err:?}"
    );
    let t = Instant::now();
    while echo.disconnects.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(echo.disconnects.load(Ordering::SeqCst), 1);
    let addr = server.local_addr();
    server.shutdown();
    assert!(BridgeClient::connect_to(addr, &token, Duration::from_millis(500)).is_err());
}

#[test]
fn the_client_never_dials_a_non_loopback_address() {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 9);
    let err =
        BridgeClient::connect_to(addr, &"a".repeat(64), Duration::from_millis(100)).unwrap_err();
    assert!(err.to_string().contains("loopback"));
}
