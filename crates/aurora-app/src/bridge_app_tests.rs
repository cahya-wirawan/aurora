//! Headless tests of the bridge's app side: options, the modal policy, the
//! command gate, the marshalling queue and the confined file staging. The
//! handlers that need a live `App` (window, GPU) are covered by the
//! Xvfb smoke script (`scripts/mcp_bridge_smoke.sh`), not here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_automation::Roots;
use aurora_automation::bridge::client::{BridgeClient, ClientError};
use aurora_automation::bridge::protocol::CallError;
use aurora_automation::bridge::server::{BridgeServer, Dispatcher, ServerConfig};
use aurora_automation::bridge::token;
use serde_json::{Value, json};

use crate::bridge_app::{AppDispatcher, BridgeOptions, Job, QUEUE_CAPACITY, policy};
use crate::bridge_doc::{command_gate, commit_export, stage_open_file};

fn env_none(_: &str) -> Option<String> {
    None
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

// ---- off by default ----

#[test]
fn the_bridge_is_off_unless_asked_for() {
    let o = BridgeOptions::from_args(&[], &env_none).unwrap();
    assert!(!o.enabled);
    assert!(o.roots.is_empty());
    // Roots or limits alone never enable it.
    let o = BridgeOptions::from_args(
        &args(&["--mcp-bridge-root", "/tmp", "--mcp-bridge-max-mpx", "5"]),
        &env_none,
    )
    .unwrap();
    assert!(!o.enabled);
    assert_eq!(o.max_pixels, 5_000_000);
    // Unknown arguments (a platform launcher's) are ignored.
    assert!(
        !BridgeOptions::from_args(&args(&["-psn_0_1", "file.png"]), &env_none)
            .unwrap()
            .enabled
    );
    // Falsy environment values stay off.
    for v in ["", "0", "false", "off", "no", "2"] {
        let env = move |n: &str| (n == "AURORA_MCP_BRIDGE").then(|| v.to_owned());
        assert!(
            !BridgeOptions::from_args(&[], &env).unwrap().enabled,
            "{v:?}"
        );
    }
}

#[test]
fn the_flag_and_the_environment_enable_it() {
    assert!(
        BridgeOptions::from_args(&args(&["--mcp-bridge"]), &env_none)
            .unwrap()
            .enabled
    );
    for v in ["1", "true", "ON", " yes "] {
        let env = move |n: &str| (n == "AURORA_MCP_BRIDGE").then(|| v.to_owned());
        assert!(
            BridgeOptions::from_args(&[], &env).unwrap().enabled,
            "{v:?}"
        );
    }
    let env = |n: &str| (n == "AURORA_MCP_BRIDGE_ROOTS").then(|| "/a:/b".to_owned());
    let o = BridgeOptions::from_args(&args(&["--mcp-bridge", "--mcp-bridge-root", "/c"]), &env)
        .unwrap();
    assert_eq!(
        o.roots,
        [
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c")
        ]
    );
}

#[test]
fn bad_option_values_are_errors() {
    assert!(BridgeOptions::from_args(&args(&["--mcp-bridge-root"]), &env_none).is_err());
    assert!(BridgeOptions::from_args(&args(&["--mcp-bridge-max-mpx", "0"]), &env_none).is_err());
    assert!(BridgeOptions::from_args(&args(&["--mcp-bridge-max-mpx", "x"]), &env_none).is_err());
}

// ---- policy ----

#[test]
fn a_modal_dialog_blocks_everything_but_reads_and_the_dialog_tool() {
    let modal = Some("Save changes?");
    for method in [
        "command.run",
        "doc.open",
        "doc.save",
        "ui.pointer",
        "ui.key",
    ] {
        let e = policy(method, None, modal, false).unwrap_err();
        assert_eq!(e.kind, "modal_dialog", "{method}");
        assert!(e.message.contains("ui_dialog"));
    }
    assert_eq!(
        policy("ui.menu", Some("activate"), modal, false)
            .unwrap_err()
            .kind,
        "modal_dialog"
    );
    for method in [
        "bridge.status",
        "command.context",
        "doc.inspect",
        "doc.preview",
        "doc.psd_check",
        "ui.inspect",
        "ui.screenshot",
        "ui.dialog",
    ] {
        assert!(policy(method, None, modal, false).is_ok(), "{method}");
    }
    assert!(policy("ui.menu", Some("list"), modal, false).is_ok());
    // No dialog: everything is allowed.
    for method in ["command.run", "doc.open", "ui.pointer", "ui.key"] {
        assert!(policy(method, None, None, false).is_ok());
    }
}

#[test]
fn an_open_palette_refuses_commands_and_files_but_not_keys() {
    for method in ["command.run", "doc.open", "doc.save"] {
        assert_eq!(
            policy(method, None, None, true).unwrap_err().kind,
            "palette_open"
        );
    }
    assert_eq!(
        policy("ui.menu", Some("activate"), None, true)
            .unwrap_err()
            .kind,
        "palette_open"
    );
    assert!(
        policy("ui.key", None, None, true).is_ok(),
        "the palette is driven by keys"
    );
    assert!(policy("ui.pointer", None, None, true).is_ok());
}

// ---- command gate ----

#[test]
fn quitting_and_file_commands_are_never_run() {
    for id in [
        "app.quit",
        "automation.disconnect",
        "file.open",
        "file.save",
    ] {
        assert_eq!(command_gate(id).unwrap_err().kind, "refused", "{id}");
    }
    // A command with a file-path parameter is refused too.
    let registry = aurora_commands::Registry::builtin();
    let with_path: Vec<String> = registry
        .list()
        .commands
        .iter()
        .filter(|c| {
            registry.get(&c.id).is_some_and(|spec| {
                spec.params
                    .iter()
                    .any(|p| matches!(p.kind, aurora_commands::ParamKind::Path { .. }))
            })
        })
        .map(|c| c.id.clone())
        .collect();
    assert!(!with_path.is_empty(), "the registry has path commands");
    for id in with_path {
        assert!(command_gate(&id).is_err(), "{id}");
    }
    for id in ["edit.undo", "select.expand", "layer.new", "tool.brush"] {
        assert!(command_gate(id).is_ok(), "{id}");
    }
}

// ---- marshalling ----

fn dispatcher(
    capacity: usize,
    wait: Duration,
) -> (Arc<AppDispatcher>, mpsc::Receiver<Job>, Arc<Mutex<u32>>) {
    let (tx, rx) = mpsc::sync_channel(capacity);
    let wakes = Arc::new(Mutex::new(0_u32));
    let w = Arc::clone(&wakes);
    let d = AppDispatcher::new(tx, Box::new(move || *w.lock().unwrap() += 1), wait);
    (Arc::new(d), rx, wakes)
}

fn request(method: &str) -> aurora_automation::bridge::protocol::Request {
    serde_json::from_value(json!({"id": 1, "method": method, "params": {}})).unwrap()
}

#[test]
fn the_queue_is_bounded_at_sixteen() {
    assert_eq!(QUEUE_CAPACITY, 16);
    let (d, rx, wakes) = dispatcher(2, Duration::from_millis(700));
    let mut waiting = Vec::new();
    for _ in 0..2 {
        let d = Arc::clone(&d);
        waiting.push(std::thread::spawn(move || {
            d.dispatch(request("ui.inspect"))
        }));
    }
    std::thread::sleep(Duration::from_millis(200));
    let full = d.dispatch(request("ui.inspect")).unwrap_err();
    assert_eq!(full.kind, "rate_limited");
    assert!(*wakes.lock().unwrap() >= 2, "the UI thread was woken");
    // The two queued ones are served by whoever drains the queue.
    while let Ok(job) = rx.try_recv() {
        job.finish(Ok(json!({"served": true})));
    }
    for w in waiting {
        assert_eq!(w.join().unwrap().unwrap()["served"], true);
    }
}

#[test]
fn a_request_the_ui_never_reached_says_it_was_not_run() {
    let (d, rx, _) = dispatcher(4, Duration::from_millis(150));
    let e = d.dispatch(request("command.run")).unwrap_err();
    assert_eq!(e.kind, "timeout");
    assert!(e.message.contains("not run"), "{}", e.message);
    let job = rx.try_recv().unwrap();
    assert!(!job.live(), "an abandoned job is skipped by the UI");
}

#[test]
fn a_request_the_ui_started_may_still_apply() {
    let (d, rx, _) = dispatcher(4, Duration::from_millis(300));
    let worker = std::thread::spawn(move || {
        let job = rx.recv().unwrap();
        job.start();
        std::thread::sleep(Duration::from_millis(600));
        job
    });
    let e = d.dispatch(request("command.run")).unwrap_err();
    assert!(e.message.contains("may still apply"), "{}", e.message);
    drop(worker.join().unwrap());
}

#[test]
fn through_a_real_socket_a_modal_dialog_refuses_a_command() {
    let (d, rx, _) = dispatcher(QUEUE_CAPACITY, Duration::from_secs(5));
    let tok = token::generate().unwrap();
    let server = BridgeServer::start(ServerConfig::default(), tok.clone(), d).unwrap();
    // A fake UI thread that applies the real policy with a dialog open.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = Arc::clone(&stop);
    let ui = std::thread::spawn(move || {
        while !s.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok(job) = rx.recv_timeout(Duration::from_millis(50)) {
                job.start();
                let action = job.param("action").and_then(Value::as_str);
                let answer = policy(&job.request.method, action, Some("Save changes?"), false)
                    .map(|()| json!({"ran": job.request.method}));
                job.finish(answer);
            }
        }
    });
    let mut client =
        BridgeClient::connect_to(server.local_addr(), &tok, Duration::from_secs(3)).unwrap();
    let e = client
        .call(
            "command.run",
            json!({"id": "edit.undo"}),
            Duration::from_secs(3),
        )
        .unwrap_err();
    assert!(
        matches!(&e, ClientError::App(c) if c.kind == "modal_dialog"),
        "{e:?}"
    );
    let ok = client
        .call(
            "ui.dialog",
            json!({"action": "get"}),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(ok["ran"], "ui.dialog");
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    ui.join().unwrap();
}

// ---- confined files ----

fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&13_u32.to_be_bytes());
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b
}

fn roots_in(dir: &std::path::Path) -> Roots {
    Roots::open(&[dir.to_path_buf()], &[dir.to_path_buf()]).unwrap()
}

#[test]
fn opening_is_confined_to_the_roots_and_copies_to_private_scratch() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.png"), png_header(10, 10)).unwrap();
    std::fs::write(outside.path().join("secret.png"), png_header(10, 10)).unwrap();
    let roots = roots_in(root.path());
    let out = outside.path().join("secret.png").display().to_string();
    for bad in [out.as_str(), "../secret.png", "/etc/passwd", "missing.png"] {
        let e = stage_open_file(&roots, &scratch.path().join("o1"), bad, 100_000_000).unwrap_err();
        assert_eq!(e.kind, "refused", "{bad}: {e}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("secret.png"),
            root.path().join("link.png"),
        )
        .unwrap();
        let e = stage_open_file(&roots, &scratch.path().join("o2"), "link.png", 100_000_000)
            .unwrap_err();
        assert_eq!(e.kind, "refused", "an escaping symlink: {e}");
    }
    assert!(
        !scratch.path().join("o1").exists(),
        "nothing staged for a refusal"
    );
    let staged = stage_open_file(&roots, &scratch.path().join("o3"), "a.png", 100_000_000).unwrap();
    assert_ne!(
        staged,
        root.path().join("a.png"),
        "the decoder gets the copy"
    );
    assert_eq!(std::fs::read(&staged).unwrap(), png_header(10, 10));
    assert_eq!(staged.file_name().unwrap(), "a.png");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(staged.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn a_huge_declared_picture_is_refused_before_anything_is_copied() {
    let root = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("bomb.png"), png_header(30_000, 30_000)).unwrap();
    let e = stage_open_file(
        &roots_in(root.path()),
        &scratch.path().join("o"),
        "bomb.png",
        50_000_000,
    )
    .unwrap_err();
    assert_eq!(e.kind, "refused");
    assert!(e.message.contains("--mcp-bridge-max-mpx"), "{}", e.message);
    assert!(!scratch.path().join("o").exists());
}

#[test]
fn with_no_roots_there_is_no_file_access() {
    let scratch = tempfile::tempdir().unwrap();
    let none = Roots::open(&[], &[]).unwrap();
    assert!(stage_open_file(&none, &scratch.path().join("o"), "/etc/passwd", 1).is_err());
    let out = scratch.path().join("out.png");
    std::fs::write(&out, b"x").unwrap();
    assert!(commit_export(&none, &out, "/tmp/x.png").is_err());
}

#[test]
fn saving_creates_a_new_file_and_never_replaces_one() {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("out.png");
    std::fs::write(&out, b"NEW-BYTES").unwrap();
    std::fs::write(root.path().join("keep.png"), b"PRECIOUS").unwrap();
    let roots = roots_in(root.path());
    let e = commit_export(&roots, &out, "keep.png").unwrap_err();
    assert_eq!(e.kind, "refused");
    assert_eq!(
        std::fs::read(root.path().join("keep.png")).unwrap(),
        b"PRECIOUS"
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("nowhere.png", root.path().join("dangling.png")).unwrap();
        assert!(commit_export(&roots, &out, "dangling.png").is_err());
        assert!(
            !root.path().join("nowhere.png").exists(),
            "a dangling link is not followed"
        );
    }
    assert!(commit_export(&roots, &out, "../escape.png").is_err());
    assert!(
        commit_export(
            &roots,
            &out,
            &work.path().join("abs.png").display().to_string()
        )
        .is_err()
    );
    assert_eq!(commit_export(&roots, &out, "fresh.png").unwrap(), 9);
    assert_eq!(
        std::fs::read(root.path().join("fresh.png")).unwrap(),
        b"NEW-BYTES"
    );
}

#[test]
fn error_kinds_survive_the_trip() {
    let c = CallError::new("modal_dialog", "x");
    assert_eq!(c.kind, "modal_dialog");
}
