//! Protocol tests: the server driven in process over a duplex stream.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::sync::atomic::Ordering;

use aurora_mcp::Limits;
use common::{Mock, dirs, start, start_with, text_json};
use serde_json::json;

const ALL_TOOLS: &[&str] = &[
    "command_batch",
    "command_list",
    "command_run",
    "doc_inspect",
    "doc_open",
    "doc_save",
    "psd_check",
    "render_preview",
];

#[tokio::test(flavor = "multi_thread")]
async fn tools_list_names_every_tool_with_all_four_hints_and_strict_schemas() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    let reply = client.request("tools/list", json!({})).await;
    let tools = reply["result"]["tools"].as_array().unwrap();
    let mut names: Vec<_> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, ALL_TOOLS);
    assert_eq!(aurora_mcp::tool_names().len(), ALL_TOOLS.len());
    for tool in tools {
        let a = &tool["annotations"];
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                a[hint].is_boolean(),
                "{} lacks {hint}: {tool}",
                tool["name"]
            );
        }
        assert!(a["title"].is_string(), "{tool}");
        assert_eq!(
            tool["inputSchema"]["additionalProperties"],
            json!(false),
            "{tool}"
        );
        assert!(tool["description"].as_str().unwrap().len() > 20);
    }
    let hint = |name: &str, key: &str| {
        tools.iter().find(|t| t["name"] == name).unwrap()["annotations"][key].clone()
    };
    for read in ["command_list", "doc_inspect", "render_preview", "psd_check"] {
        assert_eq!(hint(read, "readOnlyHint"), json!(true), "{read}");
    }
    for write in ["command_run", "command_batch", "doc_open", "doc_save"] {
        assert_eq!(hint(write, "readOnlyHint"), json!(false), "{write}");
    }
    assert_eq!(hint("command_run", "destructiveHint"), json!(true));
    assert_eq!(
        hint("doc_save", "destructiveHint"),
        json!(false),
        "doc_save never overwrites"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn resources_list_and_read_the_document_and_the_commands() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    let list = client.request("resources/list", json!({})).await;
    let uris: Vec<_> = list["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(uris, ["aurora://document", "aurora://commands"]);
    let doc = client
        .request("resources/read", json!({"uri": "aurora://document"}))
        .await;
    let text = doc["result"]["contents"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).unwrap()["document"],
        json!(null)
    );
    let commands = client
        .request("resources/read", json!({"uri": "aurora://commands"}))
        .await;
    let text = commands["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(text).unwrap()["commands"]
            .as_array()
            .unwrap()
            .len()
            > 100
    );
    let missing = client
        .request("resources/read", json!({"uri": "aurora://nope"}))
        .await;
    assert!(missing["error"].is_object(), "{missing}");
}

#[tokio::test(flavor = "multi_thread")]
async fn command_list_marks_headless_and_enabled_and_filters() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    let all = client.ok("command_list", json!({})).await;
    assert_eq!(all["document_open"], json!(false));
    let commands = all["commands"].as_array().unwrap();
    let find = |id: &str| commands.iter().find(|c| c["id"] == id).unwrap().clone();
    assert_eq!(find("layer.new")["headless"], json!(true));
    assert_eq!(
        find("layer.new")["enabled"],
        json!(false),
        "no document yet"
    );
    assert_eq!(
        find("layer.new")["disabled_reason"],
        json!("No document is open.")
    );
    assert_eq!(find("file.open")["enabled"], json!(true));
    assert_eq!(find("tool.brush")["headless"], json!(false));
    let only = client
        .ok("command_list", json!({"headless_only": true}))
        .await;
    assert!(
        only["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["headless"] == json!(true))
    );
    let filtered = client.ok("command_list", json!({"filter": "EXPAND"})).await;
    assert!(
        filtered["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "select.expand")
    );
    assert!(filtered["commands"].as_array().unwrap().len() < 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_arguments_are_32602_before_anything_runs() {
    let (r, w) = dirs();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    client.ok("doc_open", json!({"path": "a.png"})).await;
    // unknown tool argument
    assert_eq!(
        client.rpc_error("doc_inspect", json!({"bogus": 1})).await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error("command_list", json!({"filtr": "x"}))
            .await,
        -32602
    );
    // missing / wrong-typed tool arguments
    assert_eq!(client.rpc_error("doc_open", json!({})).await, -32602);
    assert_eq!(
        client.rpc_error("doc_open", json!({"path": 5})).await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error("render_preview", json!({"max_side": "big"}))
            .await,
        -32602
    );
    // unknown command id, unknown command argument, wrong type, out of range, missing
    assert_eq!(
        client
            .rpc_error("command_run", json!({"id": "no.such"}))
            .await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error("command_run", json!({"id": "layer.new", "args": {"x": 1}}))
            .await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error(
                "command_run",
                json!({"id": "select.expand", "args": {"amount": "3"}})
            )
            .await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error(
                "command_run",
                json!({"id": "select.expand", "args": {"amount": 100_000}})
            )
            .await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error("command_run", json!({"id": "select.expand"}))
            .await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error(
                "command_run",
                json!({"id": "select.expand", "args": {"amount": [1]}})
            )
            .await,
        -32602
    );
    assert_eq!(
        client
            .rpc_error(
                "command_run",
                json!({"id": "select.expand", "args": {"amount": null}})
            )
            .await,
        -32602
    );
    // unknown tool
    assert_eq!(client.rpc_error("no_such_tool", json!({})).await, -32602);
    assert!(seen.commands.lock().unwrap().is_empty(), "nothing ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn command_run_runs_refuses_and_reports() {
    let (r, _w) = dirs();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    // no document: a tool error, not a crash
    let message = client
        .tool_error("command_run", json!({"id": "layer.new"}))
        .await;
    assert!(message.contains("no document"), "{message}");
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let done = client.ok("command_run", json!({"id": "layer.new"})).await;
    assert_eq!(done["ok"], json!(true));
    assert_eq!(done["result"]["changed"], json!(true));
    // a command that needs a window
    let message = client
        .tool_error("command_run", json!({"id": "tool.brush"}))
        .await;
    assert!(
        message.contains("not available in the headless server"),
        "{message}"
    );
    // a registry requirement that does not hold (nothing to undo)
    let message = client
        .tool_error("command_run", json!({"id": "edit.undo"}))
        .await;
    assert!(message.contains("nothing to undo"), "{message}");
    // a command argument that is valid: reaches the host
    client
        .tool_error(
            "command_run",
            json!({"id": "select.expand", "args": {"amount": 3}}),
        )
        .await;
    assert_eq!(
        seen.commands.lock().unwrap().as_slice(),
        ["layer.new", "select.expand"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn command_batch_is_validated_up_front_not_atomic_and_stops_or_continues() {
    let (r, _w) = dirs();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    client.ok("doc_open", json!({"path": "a.png"})).await;
    // a bad step 2 means no step runs
    let code = client
        .rpc_error(
            "command_batch",
            json!({"steps": [{"id": "layer.new"}, {"id": "layer.new", "args": {"bad": 1}}]}),
        )
        .await;
    assert_eq!(code, -32602);
    assert!(seen.commands.lock().unwrap().is_empty());
    // empty and over-long batches
    assert_eq!(
        client
            .rpc_error("command_batch", json!({"steps": []}))
            .await,
        -32602
    );
    let many: Vec<_> = (0..33).map(|_| json!({"id": "layer.new"})).collect();
    assert_eq!(
        client
            .rpc_error("command_batch", json!({"steps": many}))
            .await,
        -32602
    );
    // default: stop at the first failing step; earlier steps stay applied
    let steps = json!({"steps": [{"id": "layer.new"}, {"id": "select.expand", "args": {"amount": 2}}, {"id": "layer.new"}]});
    let result = client.ok("command_batch", steps.clone()).await;
    assert_eq!(result["atomic"], json!(false));
    assert_eq!(result["completed"], json!(1));
    assert_eq!(result["failed"], json!(1));
    assert_eq!(result["not_run"], json!(1));
    assert_eq!(
        seen.commands.lock().unwrap().as_slice(),
        ["layer.new", "select.expand"]
    );
    // stop_on_error false: runs on
    seen.commands.lock().unwrap().clear();
    let mut cont = steps;
    cont["stop_on_error"] = json!(false);
    let result = client.ok("command_batch", cont).await;
    assert_eq!(result["completed"], json!(2));
    assert_eq!(result["failed"], json!(1));
    assert_eq!(result["not_run"], json!(0));
    assert_eq!(seen.commands.lock().unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn doc_open_stages_a_private_copy_and_doc_inspect_sees_it() {
    let (r, _w) = dirs();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    assert_eq!(
        client.ok("doc_inspect", json!({})).await["document"],
        json!(null)
    );
    std::fs::create_dir(r.path().join("sub")).unwrap();
    std::fs::write(r.path().join("sub/pic.PNG"), b"PIXELS").unwrap();
    let info = client.ok("doc_open", json!({"path": "sub/pic.PNG"})).await;
    assert_eq!(info["name"], json!("pic.PNG"));
    {
        let opened = seen.opened.lock().unwrap();
        assert_eq!(
            opened[0].1, b"PIXELS",
            "the host got the file's bytes (a copy)"
        );
    }
    // absolute path inside the root works too
    let abs = r.path().canonicalize().unwrap().join("sub/pic.PNG");
    client
        .ok("doc_open", json!({"path": abs.to_str().unwrap()}))
        .await;
    assert_eq!(
        client.ok("doc_inspect", json!({})).await["document"]["name"],
        json!("mock")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_root_means_no_file_access() {
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    let m = client
        .tool_error("doc_open", json!({"path": "/etc/passwd"}))
        .await;
    assert!(m.contains("--read-root"), "{m}");
    let m = client
        .tool_error("doc_save", json!({"path": "out.png"}))
        .await;
    assert!(m.contains("--write-root"), "{m}");
    assert!(seen.opened.lock().unwrap().is_empty());
    assert_eq!(seen.exports.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn doc_open_refuses_escapes_oversized_and_non_files() {
    let (r, _w) = dirs();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), b"SECRET").unwrap();
    std::fs::write(r.path().join("ok.png"), b"ok").unwrap();
    std::fs::create_dir(r.path().join("dir.png")).unwrap();
    let (mock, seen) = Mock::new();
    let mut client = start_with(
        mock,
        &[r.path()],
        &[],
        Limits {
            max_open_bytes: 16,
            ..Limits::default()
        },
    )
    .await;
    let secret = outside.path().join("secret.png");
    for path in [
        "../secret.png".to_owned(),
        "sub/../../secret.png".to_owned(),
        "./../x".to_owned(),
        secret.to_str().unwrap().to_owned(),
        "/etc/passwd".to_owned(),
        String::new(),
        "a\u{0}b".to_owned(),
        "bad\npath".to_owned(),
        "x".repeat(5000),
    ] {
        let m = client.tool_error("doc_open", json!({"path": path})).await;
        assert!(!m.is_empty());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(outside.path().join("secret.png"), r.path().join("link.png")).unwrap();
        symlink(outside.path(), r.path().join("dirlink")).unwrap();
        symlink("ok.png", r.path().join("inside.png")).unwrap();
        let m = client
            .tool_error("doc_open", json!({"path": "link.png"}))
            .await;
        assert!(m.contains("leaves its root"), "{m}");
        let m = client
            .tool_error("doc_open", json!({"path": "dirlink/secret.png"}))
            .await;
        assert!(m.contains("leaves its root"), "{m}");
        // a link that stays inside the root is fine
        client.ok("doc_open", json!({"path": "inside.png"})).await;
    }
    let m = client
        .tool_error("doc_open", json!({"path": "dir.png"}))
        .await;
    assert!(m.contains("not a regular file"), "{m}");
    std::fs::write(r.path().join("big.png"), vec![0_u8; 64]).unwrap();
    let m = client
        .tool_error("doc_open", json!({"path": "big.png"}))
        .await;
    assert!(m.contains("at most 16 bytes"), "{m}");
    let opened = seen.opened.lock().unwrap();
    assert!(
        opened.iter().all(|(_, bytes)| bytes != b"SECRET"),
        "nothing outside the root was read"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doc_save_writes_new_files_only_and_never_overwrites() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    // nothing to save yet
    assert!(
        client
            .tool_error("doc_save", json!({"path": "out.png"}))
            .await
            .contains("no document")
    );
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let saved = client.ok("doc_save", json!({"path": "out.png"})).await;
    assert_eq!(saved["saved"], json!(true));
    assert_eq!(
        std::fs::read(w.path().join("out.png")).unwrap(),
        b"EXPORTED-BYTES"
    );
    // existing target: refused, content intact, and the export did not even start
    let before = seen.exports.load(Ordering::SeqCst);
    std::fs::write(w.path().join("keep.png"), b"PRECIOUS").unwrap();
    let m = client
        .tool_error("doc_save", json!({"path": "keep.png"}))
        .await;
    assert!(m.contains("already exists"), "{m}");
    assert_eq!(
        std::fs::read(w.path().join("keep.png")).unwrap(),
        b"PRECIOUS"
    );
    assert_eq!(
        seen.exports.load(Ordering::SeqCst),
        before,
        "refused before the expensive export"
    );
    // saving over the file just written is refused too
    assert!(
        client
            .tool_error("doc_save", json!({"path": "out.png"}))
            .await
            .contains("already exists")
    );
    assert_eq!(
        std::fs::read(w.path().join("out.png")).unwrap(),
        b"EXPORTED-BYTES"
    );
    // a symbolic link at the target, to an existing file and dangling
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let victim = tempfile::tempdir().unwrap();
        std::fs::write(victim.path().join("v.png"), b"VICTIM").unwrap();
        symlink(victim.path().join("v.png"), w.path().join("l1.png")).unwrap();
        symlink(victim.path().join("missing.png"), w.path().join("l2.png")).unwrap();
        for name in ["l1.png", "l2.png"] {
            let m = client.tool_error("doc_save", json!({"path": name})).await;
            assert!(m.contains("already exists"), "{name}: {m}");
        }
        assert_eq!(
            std::fs::read(victim.path().join("v.png")).unwrap(),
            b"VICTIM"
        );
        assert!(!victim.path().join("missing.png").exists());
        // a symlinked folder leading out of the write root
        symlink(victim.path(), w.path().join("outdir")).unwrap();
        let m = client
            .tool_error("doc_save", json!({"path": "outdir/new.png"}))
            .await;
        assert!(m.contains("leaves its root"), "{m}");
        assert!(!victim.path().join("new.png").exists());
    }
    // escapes and bad targets
    let outside = tempfile::tempdir().unwrap();
    let abs = outside.path().join("abs.png");
    for path in [
        "../esc.png",
        "sub/../../esc.png",
        abs.to_str().unwrap(),
        "noext",
        "bad.xyz",
        "nofolder/x.png",
        "",
    ] {
        let m = client.tool_error("doc_save", json!({"path": path})).await;
        assert!(!m.is_empty(), "{path}");
    }
    assert!(!outside.path().join("abs.png").exists());
    assert!(!w.path().parent().unwrap().join("esc.png").exists());
    // formats by extension
    for name in ["f.jpg", "f.tiff", "f.aur", "f.PSD"] {
        let saved = client.ok("doc_save", json!({"path": name})).await;
        assert_eq!(saved["saved"], json!(true), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lossy_psd_needs_accept_losses() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    let (mut mock, _) = Mock::new();
    mock.lossy_psd = true;
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let held = client.ok("doc_save", json!({"path": "x.psd"})).await;
    assert_eq!(held["saved"], json!(false));
    assert_eq!(held["needs_confirmation"], json!(true));
    assert!(
        !w.path().join("x.psd").exists(),
        "nothing written without consent"
    );
    let saved = client
        .ok("doc_save", json!({"path": "x.psd", "accept_losses": true}))
        .await;
    assert_eq!(saved["saved"], json!(true));
    assert!(w.path().join("x.psd").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_an_export_leaves_no_file_and_reports_progress() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    let (mut mock, _) = Mock::new();
    mock.hang_on_export = true;
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let id = client
        .send_request(
            "tools/call",
            json!({"name": "doc_save", "arguments": {"path": "slow.png"}, "_meta": {"progressToken": "p1"}}),
        )
        .await;
    // wait for the first progress notification, then cancel
    let progress = loop {
        let message = client.next_message().await;
        if message["method"] == "notifications/progress" {
            break message;
        }
    };
    assert_eq!(progress["params"]["progressToken"], json!("p1"));
    assert_eq!(progress["params"]["total"], json!(1000.0));
    client
        .notify("notifications/cancelled", json!({"requestId": id}))
        .await;
    // the server is still alive and answers the next request
    let reply = client.request("ping", json!({})).await;
    assert!(reply.get("error").is_none(), "{reply}");
    let t = std::time::Instant::now();
    while std::fs::read_dir(w.path()).unwrap().count() > 0 {
        assert!(t.elapsed().as_secs() < 10, "a partial file was left");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!w.path().join("slow.png").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn render_preview_returns_an_image_and_bounds_its_size() {
    let (r, _w) = dirs();
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    assert!(
        client
            .tool_error("render_preview", json!({}))
            .await
            .contains("no document")
    );
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let reply = client.call("render_preview", json!({"max_side": 2})).await;
    let content = reply["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], json!("image"));
    assert_eq!(content[0]["mimeType"], json!("image/png"));
    assert!(content[0]["data"].as_str().unwrap().len() > 4);
    let note = text_json(&json!({"result": {"content": [content[1].clone()]}}));
    assert_eq!(note["width"], json!(2));
    for bad in [0, 2049, 100_000] {
        assert_eq!(
            client
                .rpc_error("render_preview", json!({"max_side": bad}))
                .await,
            -32602
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn psd_check_reports_without_a_file() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    assert!(
        client
            .tool_error("psd_check", json!({}))
            .await
            .contains("no document")
    );
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let report = client.ok("psd_check", json!({})).await;
    assert_eq!(report["asks"], json!(false));
    assert_eq!(std::fs::read_dir(w.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_line_is_32700_and_the_next_line_still_works() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    client.send_raw("this is not json").await;
    let error = client.next_message().await;
    assert_eq!(error["error"]["code"], json!(-32700));
    assert_eq!(error["id"], json!(null));
    client.send_raw("{\"jsonrpc\":\"2.0\",\"id\":").await;
    assert_eq!(client.next_message().await["error"]["code"], json!(-32700));
    let reply = client.request("tools/list", json!({})).await;
    assert!(reply["result"]["tools"].is_array());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_request_ends_the_session_and_never_runs() {
    let (r, _w) = dirs();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    let big = "a".repeat(aurora_mcp::MAX_REQUEST_BYTES + 10);
    client
        .send_raw(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{{\"name\":\"doc_open\",\"arguments\":{{\"path\":\"{big}\"}}}}}}"
        ))
        .await;
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), client.server)
        .await
        .expect("the session ends")
        .unwrap();
    assert!(result.unwrap_err().to_string().contains("exceeds"));
    assert!(seen.opened.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn every_output_line_is_a_json_rpc_message() {
    // The driver asserts "jsonrpc": "2.0" on every line it reads; run a mixed
    // session through it, including errors, and check the last reply too.
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), b"x").unwrap();
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let _ = client.call("doc_save", json!({"path": "../x.png"})).await;
    let _ = client.call("nope", json!({})).await;
    client.send_raw("garbage").await;
    let _ = client.next_message().await;
    let reply = client.request("ping", json!({})).await;
    assert_eq!(reply["jsonrpc"], json!("2.0"));
}
