//! Bridge mode: the MCP tools against a fake app (a real `BridgeServer`
//! on loopback with a recording dispatcher), no window involved.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::sync::{Arc, Mutex};

use aurora_automation::bridge::discovery::{self, Discovery};
use aurora_automation::bridge::protocol::{CallError, Request};
use aurora_automation::bridge::server::{BridgeServer, Dispatcher, ServerConfig};
use aurora_automation::bridge::token;
use aurora_mcp::{BridgeSession, DiscoverySource, Limits};
use common::{Mock, dirs, start, start_session};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeApp {
    calls: Mutex<Vec<(String, Value)>>,
    /// A canned error for a method.
    fail: Mutex<Option<(String, CallError)>>,
}

impl Dispatcher for FakeApp {
    fn dispatch(&self, request: Request) -> Result<Value, CallError> {
        self.calls
            .lock()
            .unwrap()
            .push((request.method.clone(), request.params.clone()));
        if let Some((method, error)) = self.fail.lock().unwrap().clone()
            && method == request.method
        {
            return Err(error);
        }
        Ok(match request.method.as_str() {
            "command.context" => json!({"can_undo": true}),
            "doc.inspect" => json!({"document": {"name": "live"}}),
            "ui.inspect" => json!({"nodes": [], "truncated": false}),
            "ui.screenshot" => {
                json!({"png_base64": "iVBORw0KGgo=", "width": 2, "height": 2, "scale_factor": 1.0})
            }
            "doc.preview" => {
                json!({"png_base64": "iVBORw0KGgo=", "width": 2, "height": 2, "document_width": 4, "document_height": 4})
            }
            _ => json!({"ok": true, "echo": request.params}),
        })
    }
}

struct Rig {
    app: Arc<FakeApp>,
    server: BridgeServer,
    _dir: tempfile::TempDir,
    file: std::path::PathBuf,
}

fn rig() -> Rig {
    let app = Arc::new(FakeApp::default());
    let token = token::generate().unwrap();
    let server = BridgeServer::start(ServerConfig::default(), token.clone(), app.clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let published = discovery::publish(
        &dir.path().join("b"),
        &Discovery::new(server.local_addr().port(), token, "test"),
    )
    .unwrap();
    let file = published.path().to_path_buf();
    std::mem::forget(published); // the lock must outlive the test; the tempdir removes the files
    Rig {
        app,
        server,
        _dir: dir,
        file,
    }
}

async fn bridge_client(
    rig: &Rig,
    read: &[&std::path::Path],
    write: &[&std::path::Path],
) -> common::Client {
    start_session(
        Box::new(BridgeSession::new(DiscoverySource::File(rig.file.clone()))),
        read,
        write,
        Limits::default(),
    )
    .await
}

fn names(reply: &Value) -> Vec<String> {
    let mut v: Vec<String> = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_tools_exist_only_in_bridge_mode() {
    let (read, write) = dirs();
    let r = rig();
    let mut bridge = bridge_client(&r, &[read.path()], &[write.path()]).await;
    let tools = names(&bridge.request("tools/list", json!({})).await);
    for t in [
        "ui_inspect",
        "ui_screenshot",
        "ui_pointer",
        "ui_key",
        "ui_menu",
        "ui_dialog",
    ] {
        assert!(tools.contains(&t.to_owned()), "{t} missing: {tools:?}");
    }
    let (mock, _) = Mock::new();
    let mut headless = start(mock, &[read.path()], &[write.path()]).await;
    let tools = names(&headless.request("tools/list", json!({})).await);
    assert!(tools.iter().all(|t| !t.starts_with("ui_")), "{tools:?}");
    assert_eq!(headless.rpc_error("ui_inspect", json!({})).await, -32602);
}

#[tokio::test(flavor = "multi_thread")]
async fn ui_hints_are_honest() {
    let (read, write) = dirs();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    let reply = c.request("tools/list", json!({})).await;
    for tool in reply["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let a = &tool["annotations"];
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(a[hint].is_boolean(), "{name} lacks {hint}");
        }
        let read_only = matches!(
            name,
            "ui_inspect"
                | "ui_screenshot"
                | "doc_inspect"
                | "command_list"
                | "render_preview"
                | "psd_check"
        );
        if name.starts_with("ui_") {
            assert_eq!(a["readOnlyHint"], json!(read_only), "{name}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_and_ui_calls_reach_the_app() {
    let (read, write) = dirs();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    assert_eq!(
        c.ok("doc_inspect", json!({})).await["document"]["name"],
        "live"
    );
    c.ok("ui_inspect", json!({"include_hidden": true})).await;
    let shot = c.call("ui_screenshot", json!({"max_side": 100})).await;
    assert_eq!(shot["result"]["content"][0]["type"], "image");
    c.ok("ui_pointer", json!({"action": "click", "x": 5, "y": 6}))
        .await;
    c.ok("ui_key", json!({"key": "Enter"})).await;
    c.ok("ui_menu", json!({})).await;
    c.ok("ui_dialog", json!({})).await;
    let methods: Vec<String> = r
        .app
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.0.clone())
        .collect();
    assert_eq!(
        methods,
        [
            "doc.inspect",
            "ui.inspect",
            "ui.screenshot",
            "ui.pointer",
            "ui.key",
            "ui.menu",
            "ui.dialog"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_ui_arguments_are_rejected_before_anything_is_sent() {
    let (read, write) = dirs();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    assert_eq!(
        c.rpc_error("ui_pointer", json!({"action": "teleport", "x": 1, "y": 1}))
            .await,
        -32602
    );
    assert_eq!(
        c.rpc_error("ui_pointer", json!({"action": "click"})).await,
        -32602
    );
    assert_eq!(
        c.rpc_error(
            "ui_pointer",
            json!({"action": "click", "x": 1, "y": 1, "bogus": 1})
        )
        .await,
        -32602
    );
    assert_eq!(c.rpc_error("ui_key", json!({})).await, -32602);
    assert_eq!(
        c.rpc_error("ui_menu", json!({"action": "activate"})).await,
        -32602
    );
    assert_eq!(
        c.rpc_error("ui_dialog", json!({"action": "choose"})).await,
        -32602
    );
    assert_eq!(
        c.rpc_error("ui_screenshot", json!({"max_side": "big"}))
            .await,
        -32602
    );
    assert!(r.app.calls.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn command_run_is_validated_here_and_forwarded_with_its_arguments() {
    let (read, write) = dirs();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    assert_eq!(
        c.rpc_error(
            "command_run",
            json!({"id": "select.expand", "args": {"amount": "x"}})
        )
        .await,
        -32602
    );
    assert_eq!(
        c.rpc_error("command_run", json!({"id": "no.such"})).await,
        -32602
    );
    assert!(
        r.app.calls.lock().unwrap().is_empty(),
        "nothing sent for a bad call"
    );
    c.ok(
        "command_run",
        json!({"id": "select.expand", "args": {"amount": 4}}),
    )
    .await;
    let calls = r.app.calls.lock().unwrap().clone();
    let run = calls.iter().find(|c| c.0 == "command.run").unwrap();
    assert_eq!(run.1["id"], "select.expand");
    assert_eq!(run.1["args"]["amount"], 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn quitting_is_not_a_tool() {
    let (read, write) = dirs();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    let error = c.tool_error("command_run", json!({"id": "app.quit"})).await;
    assert!(error.contains("not available over the bridge"), "{error}");
    let error = c
        .tool_error("command_run", json!({"id": "automation.disconnect"}))
        .await;
    assert!(error.contains("not available"), "{error}");
    let list = c.ok("command_list", json!({})).await;
    let quit = list["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "app.quit")
        .unwrap();
    assert_eq!(quit["enabled"], json!(false));
    assert!(
        r.app
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|c| c.0 != "command.run")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn files_are_confined_here_before_the_app_is_asked() {
    let (read, write) = dirs();
    std::fs::write(read.path().join("a.png"), b"x").unwrap();
    std::fs::write(write.path().join("exists.png"), b"x").unwrap();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    for bad in ["../x.png", "/etc/passwd", "nope.png"] {
        let e = c.tool_error("doc_open", json!({"path": bad})).await;
        assert!(!e.is_empty());
    }
    let e = c
        .tool_error("doc_save", json!({"path": "exists.png"}))
        .await;
    assert!(
        e.contains("never overwrites") || e.contains("exists"),
        "{e}"
    );
    let e = c
        .tool_error("doc_save", json!({"path": "../out.png"}))
        .await;
    assert!(!e.is_empty());
    assert!(
        r.app.calls.lock().unwrap().is_empty(),
        "nothing reached the app"
    );
    c.ok("doc_open", json!({"path": "a.png"})).await;
    c.ok("doc_save", json!({"path": "new.png"})).await;
    let calls = r.app.calls.lock().unwrap().clone();
    let open = calls.iter().find(|c| c.0 == "doc.open").unwrap();
    assert!(open.1["path"].as_str().unwrap().ends_with("a.png"));
    assert!(std::path::Path::new(open.1["path"].as_str().unwrap()).is_absolute());
    assert!(calls.iter().any(|c| c.0 == "doc.save"));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_roots_means_no_file_tools_even_over_the_bridge() {
    let r = rig();
    let mut c = bridge_client(&r, &[], &[]).await;
    assert!(
        c.tool_error("doc_open", json!({"path": "a.png"}))
            .await
            .contains("read root")
    );
    assert!(
        c.tool_error("doc_save", json!({"path": "a.png"}))
            .await
            .contains("write root")
    );
    assert!(r.app.calls.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_refusal_is_a_tool_error_with_its_words() {
    let (read, write) = dirs();
    let r = rig();
    *r.app.fail.lock().unwrap() = Some((
        "ui.pointer".into(),
        CallError::new(
            "modal_dialog",
            "a dialog is open (Save changes?); answer it with ui_dialog first",
        ),
    ));
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    let e = c
        .tool_error("ui_pointer", json!({"action": "click", "x": 1, "y": 1}))
        .await;
    assert!(e.contains("modal_dialog") && e.contains("ui_dialog"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_is_remade_for_a_read_but_not_resent_for_an_edit() {
    let (read, write) = dirs();
    let r = rig();
    let mut c = bridge_client(&r, &[read.path()], &[write.path()]).await;
    c.ok("doc_inspect", json!({})).await;
    r.server.disconnect_controller();
    std::thread::sleep(std::time::Duration::from_millis(300));
    // A read heals itself.
    assert_eq!(
        c.ok("doc_inspect", json!({})).await["document"]["name"],
        "live"
    );
    r.server.disconnect_controller();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let e = c.tool_error("ui_key", json!({"key": "a"})).await;
    assert!(
        e.contains("may have completed") || e.contains("connection"),
        "{e}"
    );
    let sent_keys = r
        .app
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.0 == "ui.key")
        .count();
    assert_eq!(sent_keys, 0, "an edit is never silently resent");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_app_the_error_says_how_to_start_one() {
    let (read, write) = dirs();
    let dir = tempfile::tempdir().unwrap();
    let mut c = start_session(
        Box::new(BridgeSession::new(DiscoverySource::Dir(
            dir.path().to_path_buf(),
        ))),
        &[read.path()],
        &[write.path()],
        Limits::default(),
    )
    .await;
    let e = c.tool_error("doc_inspect", json!({})).await;
    assert!(e.contains("--mcp-bridge"), "{e}");
}
