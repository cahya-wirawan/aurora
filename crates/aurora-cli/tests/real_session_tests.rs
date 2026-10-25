//! `aurora-cli mcp` against the real headless document session: files are
//! opened, edited through registry commands, rendered and saved, in
//! process over a duplex stream, plus the binary itself for stdout hygiene.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aurora_cli::host::AppHost;
use aurora_mcp::{Config, Limits, Roots};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Client {
    to_server: tokio::io::DuplexStream,
    lines: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    next_id: i64,
}

async fn start(read: &Path, write: &Path) -> Client {
    let roots = Roots::open(&[read.to_path_buf()], &[write.to_path_buf()]).unwrap();
    let (to_server, server_in) = tokio::io::duplex(1 << 20);
    let (server_out, from_server) = tokio::io::duplex(1 << 22);
    let config = Config {
        roots,
        limits: Limits::default(),
        scratch_parent: None,
    };
    tokio::spawn(aurora_mcp::serve_io(
        Box::new(AppHost::new()),
        config,
        server_in,
        server_out,
    ));
    let mut client = Client {
        to_server,
        lines: BufReader::new(from_server).lines(),
        next_id: 1,
    };
    client
        .request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
        )
        .await;
    client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    client
}

impl Client {
    async fn send(&mut self, value: &Value) {
        self.to_server.write_all(value.to_string().as_bytes()).await.unwrap();
        self.to_server.write_all(b"\n").await.unwrap();
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await;
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(120), self.lines.next_line())
                .await
                .expect("reply in time")
                .unwrap()
                .expect("stream open");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == json!(id) {
                return value;
            }
        }
    }
    async fn raw(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments})).await
    }
    async fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.raw(tool, arguments).await;
        assert!(reply.get("error").is_none() && reply["result"]["isError"] != json!(true), "{reply}");
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    async fn err(&mut self, tool: &str, arguments: Value) -> String {
        let reply = self.raw(tool, arguments).await;
        assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
        reply["result"]["content"][0]["text"].as_str().unwrap().to_owned()
    }
}

fn write_png(path: &Path, w: u32, h: u32) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&[(x * 4 % 256) as u8, (y * 4 % 256) as u8, 128, 255]);
        }
    }
    writer.write_image_data(&data).unwrap();
}

fn aur_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../aurora-io/tests/fixtures/aur/rich.aur")
}

fn layer_count(doc: &Value) -> u64 {
    doc["document"]["layer_count"].as_u64().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn open_inspect_edit_undo_preview_and_save_a_png() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("a.png"), 64, 48);
    let mut c = start(dir.path(), dir.path()).await;
    assert_eq!(c.ok("doc_inspect", json!({})).await["document"], json!(null));
    let info = c.ok("doc_open", json!({"path": "a.png"})).await;
    assert_eq!((info["width"].as_u64(), info["height"].as_u64()), (Some(64), Some(48)));
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(layer_count(&doc), 1);
    assert_eq!(doc["document"]["canvas"], json!({"width": 64, "height": 48}));

    // edit through registry commands; each is one history step
    c.ok("command_run", json!({"id": "layer.new"})).await;
    c.ok("command_run", json!({"id": "layer.new_group"})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(layer_count(&doc), 3);
    assert_eq!(doc["document"]["history"]["steps"], json!(["Added layer \"Layer 1\"", "Added group \"Group 1\""]));
    c.ok("command_run", json!({"id": "edit.undo"})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(layer_count(&doc), 2);
    assert_eq!(doc["document"]["history"]["undone"], json!(["Added group \"Group 1\""]));
    c.ok("command_run", json!({"id": "edit.redo"})).await;
    assert_eq!(layer_count(&c.ok("doc_inspect", json!({})).await), 3);
    c.ok("command_run", json!({"id": "layer.delete"})).await;
    assert_eq!(layer_count(&c.ok("doc_inspect", json!({})).await), 2);

    // selection commands, one undo step each, and Select > Modify with args
    c.ok("command_run", json!({"id": "select.all"})).await;
    c.ok("command_run", json!({"id": "select.contract", "args": {"amount": 4}})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    let bounds = &doc["document"]["selection"]["active"]["bounds"];
    assert_eq!((bounds["x"].as_i64(), bounds["width"].as_i64()), (Some(4), Some(56)));
    c.ok("command_run", json!({"id": "edit.undo"})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(doc["document"]["selection"]["active"]["bounds"]["width"], json!(64));
    c.ok("command_run", json!({"id": "select.feather", "args": {"radius": 2.5}})).await;
    c.ok("command_run", json!({"id": "select.deselect"})).await;
    assert_eq!(c.ok("doc_inspect", json!({})).await["document"]["selection"]["active"], json!(null));
    // refused: nothing selected
    let message = c.err("command_run", json!({"id": "select.expand", "args": {"amount": 2}})).await;
    assert!(message.contains("no selection"), "{message}");
    // needs a window
    assert!(c.err("command_run", json!({"id": "edit.copy"})).await.contains("not available"));

    // preview: a real PNG within the requested side
    let reply = c.raw("render_preview", json!({"max_side": 32})).await;
    let content = &reply["result"]["content"];
    assert_eq!(content[0]["type"], json!("image"));
    let png_bytes = base64_decode(content[0]["data"].as_str().unwrap());
    assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
    let note: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!((note["width"].as_u64(), note["height"].as_u64()), (Some(32), Some(24)));
    assert_eq!(note["document_width"], json!(64));

    // save: PNG, JPEG, TIFF, .aur and PSD, all new files; then reopen two of them
    for name in ["out.png", "out.jpg", "out.tif", "out.aur", "out.psd"] {
        let saved = c.ok("doc_save", json!({"path": name})).await;
        assert_eq!(saved["saved"], json!(true), "{name}: {saved}");
        assert!(dir.path().join(name).metadata().unwrap().len() > 100, "{name}");
    }
    assert!(c.err("doc_save", json!({"path": "out.png"})).await.contains("already exists"));
    let reopened = c.ok("doc_open", json!({"path": "out.aur"})).await;
    assert_eq!(reopened["format"], json!("aur"));
    assert_eq!(layer_count(&c.ok("doc_inspect", json!({})).await), 2);
    let reopened = c.ok("doc_open", json!({"path": "out.psd"})).await;
    assert_eq!(reopened["format"], json!("psd"));
    assert_eq!(reopened["width"], json!(64));
    // the PSD check on a plain document reports nothing to lose
    let report = c.ok("psd_check", json!({})).await;
    assert_eq!(report["asks"], json!(false), "{report}");
    assert!(report["message"].as_str().unwrap().contains("Photoshop"));
}

fn base64_decode(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(text).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_aur_file_opens_and_lists_its_layers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(aur_fixture(), dir.path().join("rich.aur")).unwrap();
    let mut c = start(dir.path(), dir.path()).await;
    let info = c.ok("doc_open", json!({"path": "rich.aur"})).await;
    assert_eq!(info["format"], json!("aur"));
    let doc = c.ok("doc_inspect", json!({})).await;
    assert!(layer_count(&doc) >= 1);
    assert!(doc["document"]["layers"].as_array().unwrap().iter().all(|l| l["id"].is_number()));
    let commands = c.ok("command_list", json!({"enabled_only": true})).await;
    assert!(commands["commands"].as_array().unwrap().iter().any(|x| x["id"] == "layer.new"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_psd_from_the_corpus_gives_a_grouped_report() {
    let psd = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/masks3.psd");
    if !psd.exists() {
        println!("SKIPPED: {} is not present", psd.display());
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::copy(&psd, dir.path().join("m.psd")).unwrap();
    let mut c = start(dir.path(), out.path()).await;
    let info = c.ok("doc_open", json!({"path": "m.psd"})).await;
    assert_eq!(info["format"], json!("psd"));
    let report = c.ok("psd_check", json!({})).await;
    assert!(report["groups"].is_array() && report["summary"].is_string() || report["groups"] == json!([]));
    // a save that would ask is held back, then accepted explicitly
    let held = c.ok("doc_save", json!({"path": "m2.psd"})).await;
    if held["saved"] == json!(false) {
        assert_eq!(held["needs_confirmation"], json!(true));
        assert!(!out.path().join("m2.psd").exists());
        let saved = c.ok("doc_save", json!({"path": "m2.psd", "accept_losses": true})).await;
        assert_eq!(saved["saved"], json!(true));
    } else {
        assert!(out.path().join("m2.psd").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_files_are_errors_not_crashes() {
    let dir = tempfile::tempdir().unwrap();
    let garbage: Vec<u8> = (0..4096_u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    for name in ["x.png", "x.jpg", "x.tif", "x.psd", "x.psb", "x.aur"] {
        std::fs::write(dir.path().join(name), &garbage).unwrap();
    }
    // a truncated real PSD header and an empty file
    std::fs::write(dir.path().join("t.psd"), b"8BPS\x00\x01").unwrap();
    std::fs::write(dir.path().join("e.png"), b"").unwrap();
    std::fs::write(dir.path().join("noext"), &garbage).unwrap();
    let mut c = start(dir.path(), dir.path()).await;
    for name in ["x.png", "x.jpg", "x.tif", "x.psd", "x.psb", "x.aur", "t.psd", "e.png", "noext"] {
        let reply = c.raw("doc_open", json!({"path": name})).await;
        assert_eq!(reply["result"]["isError"], json!(true), "{name}: {reply}");
    }
    // the server survives and still works
    write_png(&dir.path().join("a.png"), 8, 8);
    c.ok("doc_open", json!({"path": "a.png"})).await;
}

fn run_binary(args: &[&str], input: &str) -> (Vec<String>, String, Option<i32>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aurora-cli"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    (
        stdout.lines().map(str::to_owned).collect(),
        String::from_utf8(output.stderr).unwrap(),
        output.status.code(),
    )
}

#[test]
fn stdout_carries_only_json_rpc_and_logs_go_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("a.png"), 8, 8);
    let root = dir.path().to_str().unwrap();
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "not json at all",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"doc_open","arguments":{"path":"a.png"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"bogus"}}}"#,
    ]
    .join("\n")
        + "\n";
    let (lines, stderr, code) = run_binary(&["mcp", "--read-root", root], &input);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(lines.len() >= 4, "{lines:?}");
    for line in &lines {
        let value: Value = serde_json::from_str(line).unwrap_or_else(|_| panic!("not JSON on stdout: {line}"));
        assert_eq!(value["jsonrpc"], json!("2.0"), "{line}");
    }
    assert!(lines.iter().any(|l| l.contains("-32700")));
    assert!(lines.iter().any(|l| l.contains("-32602")));
}

#[test]
fn bad_command_lines_print_usage_to_stderr_only() {
    for args in [&["mcp", "--bogus"][..], &["mcp", "--read-root"][..], &["mcp", "--read-root", "/nonexistent-aurora-dir"][..], &["nope"][..], &[][..]] {
        let (lines, stderr, code) = run_binary(args, "");
        assert!(lines.is_empty(), "{args:?}: stdout must stay empty: {lines:?}");
        assert!(!stderr.is_empty(), "{args:?}");
        assert_eq!(code, Some(2), "{args:?}");
    }
}
