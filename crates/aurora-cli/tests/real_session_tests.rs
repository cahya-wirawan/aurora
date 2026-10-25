//! `aurora-cli mcp` against the real headless document session: files are
//! opened, edited through registry commands, rendered and saved, in
//! process over a duplex stream, plus the binary itself for stdout hygiene.

#![allow(
    clippy::too_many_lines,
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
    client
        .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    client
}

impl Client {
    async fn send(&mut self, value: &Value) {
        self.to_server
            .write_all(value.to_string().as_bytes())
            .await
            .unwrap();
        self.to_server.write_all(b"\n").await.unwrap();
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line =
                tokio::time::timeout(std::time::Duration::from_mins(2), self.lines.next_line())
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
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await
    }
    async fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.raw(tool, arguments).await;
        assert!(
            reply.get("error").is_none() && reply["result"]["isError"] != json!(true),
            "{reply}"
        );
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    async fn err(&mut self, tool: &str, arguments: Value) -> String {
        let reply = self.raw(tool, arguments).await;
        assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
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
    assert_eq!(
        c.ok("doc_inspect", json!({})).await["document"],
        json!(null)
    );
    let info = c.ok("doc_open", json!({"path": "a.png"})).await;
    assert_eq!(
        (info["width"].as_u64(), info["height"].as_u64()),
        (Some(64), Some(48))
    );
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(layer_count(&doc), 1);
    assert_eq!(
        doc["document"]["canvas"],
        json!({"width": 64, "height": 48})
    );

    // edit through registry commands; each is one history step
    c.ok("command_run", json!({"id": "layer.new"})).await;
    c.ok("command_run", json!({"id": "layer.new_group"})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(layer_count(&doc), 3);
    assert_eq!(
        doc["document"]["history"]["steps"],
        json!(["Added layer \"Layer 1\"", "Added group \"Group 1\""])
    );
    c.ok("command_run", json!({"id": "edit.undo"})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(layer_count(&doc), 2);
    assert_eq!(
        doc["document"]["history"]["undone"],
        json!(["Added group \"Group 1\""])
    );
    c.ok("command_run", json!({"id": "edit.redo"})).await;
    assert_eq!(layer_count(&c.ok("doc_inspect", json!({})).await), 3);
    c.ok("command_run", json!({"id": "layer.delete"})).await;
    assert_eq!(layer_count(&c.ok("doc_inspect", json!({})).await), 2);

    // selection commands, one undo step each, and Select > Modify with args
    c.ok("command_run", json!({"id": "select.all"})).await;
    c.ok(
        "command_run",
        json!({"id": "select.contract", "args": {"amount": 4}}),
    )
    .await;
    let doc = c.ok("doc_inspect", json!({})).await;
    let bounds = &doc["document"]["selection"]["active"]["bounds"];
    assert_eq!(
        (bounds["x"].as_i64(), bounds["width"].as_i64()),
        (Some(4), Some(56))
    );
    c.ok("command_run", json!({"id": "edit.undo"})).await;
    let doc = c.ok("doc_inspect", json!({})).await;
    assert_eq!(
        doc["document"]["selection"]["active"]["bounds"]["width"],
        json!(64)
    );
    c.ok(
        "command_run",
        json!({"id": "select.feather", "args": {"radius": 2.5}}),
    )
    .await;
    c.ok("command_run", json!({"id": "select.deselect"})).await;
    assert_eq!(
        c.ok("doc_inspect", json!({})).await["document"]["selection"]["active"],
        json!(null)
    );
    // refused: nothing selected
    let message = c
        .err(
            "command_run",
            json!({"id": "select.expand", "args": {"amount": 2}}),
        )
        .await;
    assert!(message.contains("no selection"), "{message}");
    // needs a window
    assert!(
        c.err("command_run", json!({"id": "edit.copy"}))
            .await
            .contains("not available")
    );

    // preview: a real PNG within the requested side
    let reply = c.raw("render_preview", json!({"max_side": 32})).await;
    let content = &reply["result"]["content"];
    assert_eq!(content[0]["type"], json!("image"));
    let png_bytes = base64_decode(content[0]["data"].as_str().unwrap());
    assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
    let note: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        (note["width"].as_u64(), note["height"].as_u64()),
        (Some(32), Some(24))
    );
    assert_eq!(note["document_width"], json!(64));

    // save: PNG, JPEG, TIFF, .aur and PSD, all new files; then reopen two of them
    for name in ["out.png", "out.jpg", "out.tif", "out.aur", "out.psd"] {
        let saved = c.ok("doc_save", json!({"path": name})).await;
        assert_eq!(saved["saved"], json!(true), "{name}: {saved}");
        assert!(
            dir.path().join(name).metadata().unwrap().len() > 100,
            "{name}"
        );
    }
    assert!(
        c.err("doc_save", json!({"path": "out.png"}))
            .await
            .contains("already exists")
    );
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
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
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
    assert!(
        doc["document"]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["id"].is_number())
    );
    let commands = c.ok("command_list", json!({"enabled_only": true})).await;
    assert!(
        commands["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == "layer.new")
    );
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
    assert!(
        report["groups"].is_array() && report["summary"].is_string()
            || report["groups"] == json!([])
    );
    // a save that would ask is held back, then accepted explicitly
    let held = c.ok("doc_save", json!({"path": "m2.psd"})).await;
    if held["saved"] == json!(false) {
        assert_eq!(held["needs_confirmation"], json!(true));
        assert!(!out.path().join("m2.psd").exists());
        let saved = c
            .ok("doc_save", json!({"path": "m2.psd", "accept_losses": true}))
            .await;
        assert_eq!(saved["saved"], json!(true));
    } else {
        assert!(out.path().join("m2.psd").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_files_are_errors_not_crashes() {
    let dir = tempfile::tempdir().unwrap();
    let garbage: Vec<u8> = (0..4096_u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    for name in ["x.png", "x.jpg", "x.tif", "x.psd", "x.psb", "x.aur"] {
        std::fs::write(dir.path().join(name), &garbage).unwrap();
    }
    // a truncated real PSD header and an empty file
    std::fs::write(dir.path().join("t.psd"), b"8BPS\x00\x01").unwrap();
    std::fs::write(dir.path().join("e.png"), b"").unwrap();
    std::fs::write(dir.path().join("noext"), &garbage).unwrap();
    let mut c = start(dir.path(), dir.path()).await;
    for name in [
        "x.png", "x.jpg", "x.tif", "x.psd", "x.psb", "x.aur", "t.psd", "e.png", "noext",
    ] {
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
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
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
        let value: Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("not JSON on stdout: {line}"));
        assert_eq!(value["jsonrpc"], json!("2.0"), "{line}");
    }
    assert!(lines.iter().any(|l| l.contains("-32700")));
    assert!(lines.iter().any(|l| l.contains("-32602")));
}

#[test]
fn bad_command_lines_print_usage_to_stderr_only() {
    for args in [
        &["mcp", "--bogus"][..],
        &["mcp", "--read-root"][..],
        &["mcp", "--read-root", "/nonexistent-aurora-dir"][..],
        &["nope"][..],
        &[][..],
    ] {
        let (lines, stderr, code) = run_binary(args, "");
        assert!(
            lines.is_empty(),
            "{args:?}: stdout must stay empty: {lines:?}"
        );
        assert!(!stderr.is_empty(), "{args:?}");
        assert_eq!(code, Some(2), "{args:?}");
    }
}

// ----- bombs -----------------------------------------------------------------

/// A stored-method zip with one entry called `manifest`: `declared` is the
/// size its headers claim, `data` the bytes it really holds.
fn zip_with_manifest(declared: u32, data: &[u8]) -> Vec<u8> {
    let name = b"manifest";
    let mut out = Vec::new();
    let mut local = b"PK\x03\x04".to_vec();
    local.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    local.extend_from_slice(&declared.to_le_bytes());
    local.extend_from_slice(&declared.to_le_bytes());
    local.extend_from_slice(&(name.len() as u16).to_le_bytes());
    local.extend_from_slice(&[0, 0]);
    local.extend_from_slice(name);
    out.extend_from_slice(&local);
    out.extend_from_slice(data);
    let central_at = out.len() as u32;
    let mut central = b"PK\x01\x02".to_vec();
    central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    central.extend_from_slice(&declared.to_le_bytes());
    central.extend_from_slice(&declared.to_le_bytes());
    central.extend_from_slice(&(name.len() as u16).to_le_bytes());
    central.extend_from_slice(&[0; 12]);
    central.extend_from_slice(&0_u32.to_le_bytes());
    central.extend_from_slice(name);
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn psd_with_layers(layer_count: i16, canvas: u32) -> Vec<u8> {
    let mut b = b"8BPS".to_vec();
    b.extend_from_slice(&1_u16.to_be_bytes());
    b.extend_from_slice(&[0; 6]);
    b.extend_from_slice(&3_u16.to_be_bytes());
    b.extend_from_slice(&canvas.to_be_bytes());
    b.extend_from_slice(&canvas.to_be_bytes());
    b.extend_from_slice(&8_u16.to_be_bytes());
    b.extend_from_slice(&3_u16.to_be_bytes());
    b.extend_from_slice(&0_u32.to_be_bytes()); // colour mode data
    b.extend_from_slice(&0_u32.to_be_bytes()); // image resources
    // layer and mask information: claims a huge layer count, holds nothing
    b.extend_from_slice(&64_u32.to_be_bytes());
    b.extend_from_slice(&32_u32.to_be_bytes());
    b.extend_from_slice(&layer_count.to_be_bytes());
    b.extend_from_slice(&[0; 40]);
    b
}

#[tokio::test(flavor = "multi_thread")]
async fn size_bombs_are_refused_quickly_with_bounded_memory() {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("bomb.aur", zip_with_manifest(u32::MAX, b"tiny")),
        ("lying.aur", zip_with_manifest(4, b"abcdefghijklmnop")),
        ("layers.psd", psd_with_layers(32_767, 16)),
        ("neg.psd", psd_with_layers(-32_768, 16)),
        ("psb.psb", {
            let mut b = psd_with_layers(1, 16);
            b[4..6].copy_from_slice(&2_u16.to_be_bytes());
            b[14..18].copy_from_slice(&300_000_u32.to_be_bytes());
            b[18..22].copy_from_slice(&300_000_u32.to_be_bytes());
            b
        }),
    ];
    for (name, bytes) in &files {
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    let mut c = start(dir.path(), dir.path()).await;
    let started = std::time::Instant::now();
    for (name, _) in &files {
        let message = c.err("doc_open", json!({"path": name})).await;
        assert!(!message.is_empty(), "{name}");
        assert!(
            !message.contains("FormatError {"),
            "{name}: plain words, not Debug text: {message}"
        );
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    let high_water = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(str::to_owned)
        });
    if let Some(line) = high_water {
        let kib: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!(
            kib < 4 * 1024 * 1024,
            "the test process stayed under 4 GiB: {line}"
        );
    }
    // the session still works afterwards
    write_png(&dir.path().join("a.png"), 8, 8);
    c.ok("doc_open", json!({"path": "a.png"})).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_png_header_is_refused_before_the_real_open_path() {
    let dir = tempfile::tempdir().unwrap();
    // a real 30000 x 30000 PNG header over a few real bytes
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&13_u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&30_000_u32.to_be_bytes());
    png.extend_from_slice(&30_000_u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    std::fs::write(dir.path().join("huge.png"), &png).unwrap();
    let mut c = start(dir.path(), dir.path()).await;
    let started = std::time::Instant::now();
    let message = c.err("doc_open", json!({"path": "huge.png"})).await;
    assert!(message.contains("--max-open-mpx"), "{message}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // the document was never replaced
    assert_eq!(
        c.ok("doc_inspect", json!({})).await["document"],
        json!(null)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_aur_manifest_declaring_too_many_pixels_is_refused_by_the_host() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(aur_fixture(), dir.path().join("rich.aur")).unwrap();
    let roots = Roots::open(&[dir.path().to_path_buf()], &[]).unwrap();
    let (to_server, server_in) = tokio::io::duplex(1 << 20);
    let (server_out, from_server) = tokio::io::duplex(1 << 22);
    let limits = Limits {
        max_open_pixels: 4,
        ..Limits::default()
    };
    let config = Config {
        roots,
        limits,
        scratch_parent: None,
    };
    tokio::spawn(aurora_mcp::serve_io(
        Box::new(AppHost::with_limits(&limits)),
        config,
        server_in,
        server_out,
    ));
    let mut c = Client {
        to_server,
        lines: BufReader::new(from_server).lines(),
        next_id: 1,
    };
    c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})).await;
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    let message = c.err("doc_open", json!({"path": "rich.aur"})).await;
    assert!(message.contains("manifest declares"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_scratch_dir_option_holds_the_document_store_and_staging() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("a.png"), 16, 16);
    let roots = Roots::open(&[dir.path().to_path_buf()], &[]).unwrap();
    let (to_server, server_in) = tokio::io::duplex(1 << 20);
    let (server_out, from_server) = tokio::io::duplex(1 << 22);
    let config = Config {
        roots,
        limits: Limits::default(),
        scratch_parent: Some(scratch.path().to_path_buf()),
    };
    tokio::spawn(aurora_mcp::serve_io(
        Box::new(AppHost::new()),
        config,
        server_in,
        server_out,
    ));
    let mut c = Client {
        to_server,
        lines: BufReader::new(from_server).lines(),
        next_id: 1,
    };
    c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})).await;
    c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    c.ok("doc_open", json!({"path": "a.png"})).await;
    let server_dir = std::fs::read_dir(scratch.path())
        .unwrap()
        .flatten()
        .next()
        .expect("a scratch dir")
        .path();
    let host_docs: Vec<_> = std::fs::read_dir(server_dir.join("host"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(
        host_docs.len(),
        1,
        "the document's tile store lives under --scratch-dir"
    );
}

#[test]
fn a_stray_println_does_not_reach_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("a.png"), 8, 8);
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ]
    .join("\n")
        + "\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_aurora-cli"))
        .args(["mcp", "--read-root", dir.path().to_str().unwrap()])
        .env("AURORA_MCP_SELFTEST_STRAY_STDOUT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stdout.contains("stray output"),
        "the protocol stream was polluted: {stdout}"
    );
    for line in stdout.lines() {
        let value: Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("not JSON: {line}"));
        assert_eq!(value["jsonrpc"], json!("2.0"));
    }
    assert!(stdout.lines().count() >= 2);
    #[cfg(unix)]
    assert!(
        stderr.contains("stray output"),
        "the stray line went to stderr: {stderr}"
    );
}

// ---- bridge mode through the real binary ----

mod bridge {
    use super::*;
    use aurora_automation::bridge::discovery::{self, Discovery};
    use aurora_automation::bridge::protocol::{CallError, Request};
    use aurora_automation::bridge::server::{BridgeServer, Dispatcher, ServerConfig};
    use aurora_automation::bridge::token;

    struct Fake;
    impl Dispatcher for Fake {
        fn dispatch(&self, request: Request) -> Result<Value, CallError> {
            match request.method.as_str() {
                "doc.inspect" => Ok(json!({"document": {"name": "from-the-fake-app"}})),
                _ => Err(CallError::new("refused", "no")),
            }
        }
    }

    fn bridge_binary(args: &[&str], input: &str) -> (Vec<String>, String, Option<i32>) {
        run_binary(args, input)
    }

    #[test]
    fn the_binary_proxies_a_tool_call_to_the_app_and_never_without_one() {
        let token = token::generate().unwrap();
        let server = BridgeServer::start(
            ServerConfig::default(),
            token.clone(),
            std::sync::Arc::new(Fake),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let published = discovery::publish(
            &dir.path().join("b"),
            &Discovery::new(server.local_addr().port(), token, "t"),
        )
        .unwrap();
        let file = published.path().display().to_string();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
        let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"doc_inspect","arguments":{}}}"#;
        let input = format!("{init}\n{initialized}\n{call}\n");
        let (lines, _err, _code) =
            bridge_binary(&["mcp", "--bridge", "--discovery", &file], &input);
        let reply: Value = lines
            .iter()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["id"] == 2)
            .unwrap();
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("from-the-fake-app"),
            "{reply}"
        );
        // A discovery file that does not exist: a clear tool error, no crash.
        let (lines, _err, _code) = bridge_binary(
            &[
                "mcp",
                "--bridge",
                "--discovery",
                "/nonexistent/aurora-bridge-1.json",
            ],
            &input,
        );
        let reply: Value = lines
            .iter()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["id"] == 2)
            .unwrap();
        assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
        // --discovery without --bridge is a usage error.
        let (_l, err, code) = bridge_binary(&["mcp", "--discovery", &file], "");
        assert_eq!(code, Some(2), "{err}");
    }
}
