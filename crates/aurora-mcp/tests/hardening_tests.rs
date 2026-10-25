//! Hardening tests: size bombs, responsiveness while a long operation
//! runs, lock discipline, scratch hygiene and protocol edge cases.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[cfg_attr(not(unix), allow(unused_imports))]
use aurora_mcp::{Limits, PathError, Roots, header};
use common::{Mock, dirs, start, start_with};
use serde_json::{Value, json};

// ----- header bombs --------------------------------------------------------

fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&13_u32.to_be_bytes());
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    b.extend_from_slice(&[0; 64]);
    b
}

fn psd_header(version: u16, w: u32, h: u32) -> Vec<u8> {
    let mut b = b"8BPS".to_vec();
    b.extend_from_slice(&version.to_be_bytes());
    b.extend_from_slice(&[0; 6]);
    b.extend_from_slice(&3_u16.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&8_u16.to_be_bytes());
    b.extend_from_slice(&3_u16.to_be_bytes());
    b.extend_from_slice(&[0; 32]);
    b
}

fn jpeg_header(w: u16, h: u16) -> Vec<u8> {
    let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    b.extend_from_slice(&[0; 14]);
    b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 8]);
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    b
}

fn tiff_header(big_endian: bool, w: u32, h: u32) -> Vec<u8> {
    let u16b = |v: u16| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32b = |v: u32| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let mut b = if big_endian {
        b"MM\0*".to_vec()
    } else {
        b"II*\0".to_vec()
    };
    b.extend_from_slice(&u32b(8));
    b.extend_from_slice(&u16b(2));
    for (tag, value) in [(256_u16, w), (257, h)] {
        b.extend_from_slice(&u16b(tag));
        b.extend_from_slice(&u16b(4));
        b.extend_from_slice(&u32b(1));
        b.extend_from_slice(&u32b(value));
    }
    b.extend_from_slice(&u32b(0));
    b
}

#[test]
fn header_parsing_reads_each_format() {
    let dims = |bytes: Vec<u8>| {
        header::dimensions(&mut std::io::Cursor::new(bytes))
            .unwrap()
            .unwrap()
    };
    let d = dims(png_header(30_000, 20_000));
    assert_eq!((d.width, d.height, d.format), (30_000, 20_000, "PNG"));
    let d = dims(psd_header(2, 300_000, 300_000));
    assert_eq!((d.pixels(), d.format), (90_000_000_000, "PSB"));
    assert_eq!(dims(psd_header(1, 5, 7)).format, "PSD");
    let d = dims(jpeg_header(60_000, 50_000));
    assert_eq!((d.width, d.height, d.format), (60_000, 50_000, "JPEG"));
    for big in [false, true] {
        let d = dims(tiff_header(big, 100_000, 90_000));
        assert_eq!((d.width, d.height, d.format), (100_000, 90_000, "TIFF"));
    }
    // unknown content is left to the decoders; cut headers are refused
    assert!(
        header::dimensions(&mut std::io::Cursor::new(b"hello".to_vec()))
            .unwrap()
            .is_none()
    );
    for cut in [
        b"\x89PNG\r\n\x1a\n\0\0".to_vec(),
        b"8BPS\0\x02".to_vec(),
        vec![0xFF, 0xD8, 0xFF],
        b"II*\0\x08".to_vec(),
    ] {
        assert!(header::dimensions(&mut std::io::Cursor::new(cut)).is_err());
    }
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_headers_are_refused_fast_before_the_host_is_reached() {
    let (r, _w) = dirs();
    let bombs: Vec<(&str, Vec<u8>)> = vec![
        ("png.png", png_header(30_000, 30_000)),
        ("zero.png", png_header(0, 100)),
        ("psb.psb", psd_header(2, 300_000, 300_000)),
        ("psd.psd", psd_header(1, 30_000, 30_000)),
        ("jpg.jpg", jpeg_header(60_000, 60_000)),
        ("tif.tif", tiff_header(false, 100_000, 100_000)),
        ("tifbe.tif", tiff_header(true, 100_000, 100_000)),
        // a PSB wearing a .png name is checked as a PSB
        ("liar.png", psd_header(2, 300_000, 300_000)),
        ("cut.png", b"\x89PNG\r\n\x1a\n\0\0".to_vec()),
    ];
    for (name, bytes) in &bombs {
        std::fs::write(r.path().join(name), bytes).unwrap();
    }
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    let before = peak_rss_kib();
    let started = Instant::now();
    for (name, _) in &bombs {
        let message = client.tool_error("doc_open", json!({"path": name})).await;
        assert!(!message.is_empty(), "{name}");
        if *name != "zero.png" && *name != "cut.png" {
            assert!(message.contains("--max-open-mpx"), "{name}: {message}");
        }
    }
    assert!(started.elapsed() < Duration::from_secs(5), "refused fast");
    assert!(
        seen.opened.lock().unwrap().is_empty(),
        "the host was never reached"
    );
    if let (Some(a), Some(b)) = (before, peak_rss_kib()) {
        assert!(
            b.saturating_sub(a) < 200 * 1024,
            "no allocation from a header number: {a} -> {b} KiB"
        );
    }
    // nothing was even staged
    let staged: Vec<_> = std::fs::read_dir(
        seen.scratch
            .lock()
            .unwrap()
            .clone()
            .unwrap()
            .parent()
            .unwrap(),
    )
    .unwrap()
    .flatten()
    .filter(|e| e.file_name().to_string_lossy().starts_with("open-"))
    .collect();
    assert!(staged.is_empty());
    // a modest picture, and the cap as an option
    std::fs::write(r.path().join("ok.png"), png_header(1000, 1000)).unwrap();
    client.ok("doc_open", json!({"path": "ok.png"})).await;
    drop(client);
    let (mock, _) = Mock::new();
    let mut tight = start_with(
        mock,
        &[r.path()],
        &[],
        Limits {
            max_open_pixels: 500_000,
            ..Limits::default()
        },
    )
    .await;
    assert!(
        tight
            .tool_error("doc_open", json!({"path": "ok.png"}))
            .await
            .contains("at most 0 Mpx")
    );
}

// ----- responsiveness and the session lock --------------------------------

/// With ONE runtime thread, anything that blocks it (a session lock taken on
/// an async worker, blocking work outside `spawn_blocking`) freezes the
/// server AND this client, so the ping below is never answered.
#[tokio::test(flavor = "current_thread")]
async fn a_single_runtime_thread_still_serves_ping_and_cancel_during_a_long_preview() {
    long_preview_stays_responsive().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_and_cancel_are_served_while_a_long_preview_holds_the_session() {
    long_preview_stays_responsive().await;
}

async fn long_preview_stays_responsive() {
    let (r, _w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mut mock, _seen) = Mock::new();
    mock.hang_on_preview = true;
    let mut client = start(mock, &[r.path()], &[]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let whole = Instant::now();
    let preview = client
        .send_request(
            "tools/call",
            json!({"name": "render_preview", "arguments": {}}),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // two more calls queue behind the preview; with two runtime workers
    // they must not occupy them
    let mut queued = Vec::new();
    for _ in 0..4 {
        queued.push(
            client
                .send_request(
                    "tools/call",
                    json!({"name": "command_run", "arguments": {"id": "layer.new"}}),
                )
                .await,
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    let started = Instant::now();
    let pong = client.request("ping", json!({})).await;
    assert!(pong.get("error").is_none(), "{pong}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "ping took {:?}",
        started.elapsed()
    );
    // cancel the preview: it must stop, which lets the queued calls run
    client
        .notify("notifications/cancelled", json!({"requestId": preview}))
        .await;
    let started = Instant::now();
    for id in queued {
        let reply = client.reply(id).await;
        assert_eq!(reply["result"]["isError"], json!(false), "{reply}");
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the cancel took effect: {:?}",
        started.elapsed()
    );
    // The mock preview would run for 10 s if nothing cancelled it, and a
    // blocked runtime thread freezes the client's own sleeps too: the whole
    // exchange must be short.
    assert!(
        whole.elapsed() < Duration::from_secs(4),
        "the runtime stalled: {:?}",
        whole.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commands_enabled_check_and_run_share_one_lock_hold() {
    let (r, _w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    seen.order.lock().unwrap().clear();
    let a = client
        .send_request(
            "tools/call",
            json!({"name": "command_run", "arguments": {"id": "layer.new"}}),
        )
        .await;
    let b = client
        .send_request(
            "tools/call",
            json!({"name": "command_run", "arguments": {"id": "select.all"}}),
        )
        .await;
    client.reply(a).await;
    client.reply(b).await;
    let order = seen.order.lock().unwrap().clone();
    assert_eq!(order.len(), 4, "{order:?}");
    assert_eq!(order[0], "check", "{order:?}");
    assert!(
        order[1].starts_with("run:"),
        "a check is followed by its own run: {order:?}"
    );
    assert_eq!(order[2], "check", "{order:?}");
    assert!(order[3].starts_with("run:"), "{order:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_of_host_commands_is_not_interleaved_by_other_requests() {
    let (r, _w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    seen.order.lock().unwrap().clear();
    let batch = client
        .send_request(
            "tools/call",
            json!({"name": "command_batch", "arguments": {"steps": [{"id": "layer.new"}, {"id": "layer.new"}, {"id": "layer.new"}]}}),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let single = client
        .send_request(
            "tools/call",
            json!({"name": "command_run", "arguments": {"id": "select.all"}}),
        )
        .await;
    let reply = client.reply(batch).await;
    client.reply(single).await;
    let result: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["interleaved_by_other_requests"], json!(false));
    let runs: Vec<String> = seen
        .order
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.starts_with("run:"))
        .cloned()
        .collect();
    assert_eq!(runs.len(), 4);
    let at = runs.iter().position(|e| e == "run:select.all").unwrap();
    assert!(
        at == 0 || at == 3,
        "the single command ran between batch steps: {runs:?}"
    );
    // a batch that touches files says it may be interleaved
    let mixed = client
        .ok(
            "command_batch",
            json!({"steps": [{"id": "layer.new"}, {"id": "file.open", "args": {"path": "a.png"}}]}),
        )
        .await;
    assert_eq!(mixed["interleaved_by_other_requests"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn two_saves_to_one_target_export_once() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[w.path()]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let args = json!({"name": "doc_save", "arguments": {"path": "same.png"}});
    let a = client.send_request("tools/call", args.clone()).await;
    let b = client.send_request("tools/call", args).await;
    let (ra, rb) = (client.reply(a).await, client.reply(b).await);
    let errors = [&ra, &rb]
        .iter()
        .filter(|r| r["result"]["isError"] == json!(true))
        .count();
    assert_eq!(errors, 1, "{ra} {rb}");
    assert_eq!(
        seen.exports.load(Ordering::SeqCst),
        1,
        "the loser did not run a whole export"
    );
}

// ----- export size, disk space, scratch -----------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn an_export_is_stopped_while_it_is_written_when_it_outgrows_the_limit() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mut mock, seen) = Mock::new();
    mock.slow_big_export = true;
    let mut client = start_with(
        mock,
        &[r.path()],
        &[w.path()],
        Limits {
            max_export_bytes: 256 * 1024,
            ..Limits::default()
        },
    )
    .await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let message = client
        .tool_error("doc_save", json!({"path": "big.png"}))
        .await;
    assert!(message.contains("larger than the limit"), "{message}");
    let chunks = seen.written_chunks.load(Ordering::SeqCst);
    assert!(
        chunks < 100,
        "the writer was stopped early, not after all 200 chunks (wrote {chunks})"
    );
    assert!(!w.path().join("big.png").exists());
    let scratch = seen.scratch.lock().unwrap().clone().unwrap();
    let left: Vec<_> = std::fs::read_dir(scratch.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("export-"))
        .collect();
    assert!(left.is_empty(), "the export directory is removed");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_export_that_cannot_fit_on_the_scratch_disk_is_refused_before_it_starts() {
    let (r, w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mut mock, seen) = Mock::new();
    mock.estimate = Some(1 << 60);
    let limits = Limits {
        max_export_bytes: 1 << 60,
        ..Limits::default()
    };
    let mut client = start_with(mock, &[r.path()], &[w.path()], limits).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let message = client
        .tool_error("doc_save", json!({"path": "x.png"}))
        .await;
    assert!(message.contains("scratch disk"), "{message}");
    assert_eq!(
        seen.exports.load(Ordering::SeqCst),
        0,
        "refused before the export ran"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn scratch_is_private_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let (r, _w) = dirs();
    let parent = tempfile::tempdir().unwrap();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mock, seen) = Mock::new();
    let roots = Roots::open(&[r.path().to_path_buf()], &[]).unwrap();
    let (c2s, server_in) = tokio::io::duplex(1 << 16);
    let (server_out, _from) = tokio::io::duplex(1 << 16);
    let config = aurora_mcp::Config {
        roots,
        limits: Limits::default(),
        scratch_parent: Some(parent.path().to_path_buf()),
    };
    let _server = tokio::spawn(aurora_mcp::serve_io(
        Box::new(mock),
        config,
        server_in,
        server_out,
    ));
    let _keep = c2s;
    // wait for startup to hand the host its directory
    let t = Instant::now();
    let host = loop {
        if let Some(dir) = seen.scratch.lock().unwrap().clone() {
            break dir;
        }
        assert!(t.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode(host.parent().unwrap()),
        0o700,
        "the server's scratch directory"
    );
    assert_eq!(mode(&host), 0o700, "the host's directory");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn staged_copies_are_owner_only() {
    let (r, _w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mock, seen) = Mock::new();
    let mut client = start(mock, &[r.path()], &[]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    assert_eq!(seen.staged_modes.lock().unwrap().as_slice(), [0o600]);
}

/// A directory shaped like one of the server's, with or without its marker
/// and lock file.
#[cfg(unix)]
fn fake_scratch(
    parent: &std::path::Path,
    suffix: &str,
    marker: bool,
    lock: bool,
) -> std::path::PathBuf {
    use aurora_mcp::scratch::{LOCK_FILE, MARKER_FILE, PREFIX};
    let dir = parent.join(format!("{PREFIX}{suffix}"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("open-1.psd"), vec![0_u8; 1024]).unwrap();
    if marker {
        std::fs::write(dir.join(MARKER_FILE), b"x").unwrap();
    }
    if lock {
        std::fs::write(dir.join(LOCK_FILE), b"").unwrap();
    }
    dir
}

#[cfg(unix)] // the sweep and its symlink case are Unix-only
#[test]
fn a_dead_servers_scratch_is_swept_and_nothing_else_is() {
    use aurora_mcp::scratch::{PREFIX, Scratch, sweep_stale};
    let parent = tempfile::tempdir().unwrap();
    let dead = fake_scratch(parent.path(), "AbCd1234", true, true);
    // a person's own directory that happens to start with the prefix
    let notes = parent.path().join(format!("{PREFIX}notes"));
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("todo.txt"), b"mine").unwrap();
    // right name shape but no marker (not made by this server), even with a lock file
    let markerless = fake_scratch(parent.path(), "Zz990011", false, true);
    let markerless_nolock = fake_scratch(parent.path(), "Yy880022", false, false);
    // marked, but its lock file is gone: maybe a live server's, left alone
    let no_lock = fake_scratch(parent.path(), "Xx770033", true, false);
    // right shape, wrong length
    let long = fake_scratch(parent.path(), "toolongsuffix1", true, true);
    // a plain file and a symlink wearing the name
    std::fs::write(parent.path().join(format!("{PREFIX}file1234")), b"x").unwrap();
    let precious = tempfile::tempdir().unwrap();
    std::fs::write(precious.path().join("keep"), b"x").unwrap();
    std::fs::write(precious.path().join(".aurora-mcp-scratch"), b"x").unwrap();
    std::fs::write(precious.path().join(".lock"), b"").unwrap();
    std::os::unix::fs::symlink(
        precious.path(),
        parent.path().join(format!("{PREFIX}Link1234")),
    )
    .unwrap();
    // a live server
    let live = Scratch::create(Some(parent.path())).unwrap();
    assert!(
        !dead.exists(),
        "the marked dead server's directory was swept"
    );
    for survivor in [&notes, &markerless, &markerless_nolock, &no_lock, &long] {
        assert!(survivor.exists(), "{} must survive", survivor.display());
    }
    assert_eq!(std::fs::read(notes.join("todo.txt")).unwrap(), b"mine");
    assert!(
        precious.path().join("keep").exists(),
        "a symlink is never followed"
    );
    assert!(live.path().exists());
    // a live server survives even when a tmp cleaner removed its lock file
    std::fs::remove_file(live.path().join(".lock")).unwrap();
    let second = Scratch::create(Some(parent.path())).unwrap();
    assert!(live.path().exists() && second.path().exists());
    assert_eq!(sweep_stale(parent.path()), 0);
    let gone = live.path().to_path_buf();
    drop(live);
    assert!(!gone.exists(), "a clean exit removes the directory");
    // the real names have the exact shape the sweep requires
    let name = second
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(name.len(), PREFIX.len() + aurora_mcp::scratch::RANDOM_LEN);
}

#[test]
fn a_starting_servers_directory_is_never_swept_at_any_instant_of_its_creation() {
    use aurora_mcp::scratch::{Scratch, sweep_stale};
    let parent = tempfile::tempdir().unwrap();
    let seen = std::cell::Cell::new(0);
    let server = Scratch::create_with(Some(parent.path()), &|dir| {
        // a second server starting at exactly this instant
        assert_eq!(
            sweep_stale(parent.path()),
            0,
            "swept a starting server's directory"
        );
        assert!(dir.exists());
        seen.set(seen.get() + 1);
    })
    .unwrap();
    assert_eq!(seen.get(), 4, "every step was probed");
    assert!(server.path().exists());
}

#[test]
fn a_sweep_holds_the_directorys_lock_while_it_removes_it() {
    use aurora_mcp::scratch::{LOCK_FILE, MARKER_FILE, PREFIX, sweep_dir};
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join(format!("{PREFIX}AbCd1234"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(MARKER_FILE), b"x").unwrap();
    std::fs::write(dir.join(LOCK_FILE), b"").unwrap();
    let probed = std::cell::Cell::new(false);
    assert!(sweep_dir(&dir, &|| {
        // an owner trying to lock the directory right now must fail
        let other = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .unwrap();
        assert!(
            other.try_lock().is_err(),
            "the sweep did not hold the lock during removal"
        );
        probed.set(true);
    }));
    assert!(probed.get() && !dir.exists());
}

// ----- protocol edge cases --------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_non_utf8_line_is_32700_and_the_session_continues() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    client.send_bytes(&[0xFF, 0xFE, 0x80, b'\n']).await;
    let error = client.next_message().await;
    assert_eq!(error["error"]["code"], json!(-32700));
    assert_eq!(error["id"], json!(null));
    let reply = client.request("tools/list", json!({})).await;
    assert!(reply["result"]["tools"].is_array());
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_requests_get_32600_with_an_id_field() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    for line in [
        "[]",
        "42",
        "\"x\"",
        "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]",
    ] {
        client.send_raw(line).await;
        let reply = client.next_message().await;
        assert_eq!(reply["error"]["code"], json!(-32600), "{line}: {reply}");
        assert!(reply.as_object().unwrap().contains_key("id"), "{reply}");
        assert_eq!(reply["id"], json!(null));
    }
    client.send_raw("{\"jsonrpc\":\"2.0\",\"id\":77}").await;
    let reply = client.next_message().await;
    assert_eq!(
        (reply["error"]["code"].as_i64(), reply["id"].as_i64()),
        (Some(-32600), Some(77))
    );
    assert!(
        client
            .request("ping", json!({}))
            .await
            .get("error")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_id_already_in_flight_is_rejected_not_lost() {
    let (r, _w) = dirs();
    std::fs::write(r.path().join("a.png"), png_header(10, 10)).unwrap();
    let (mut mock, _) = Mock::new();
    mock.hang_on_preview = true;
    let mut client = start(mock, &[r.path()], &[]).await;
    client.ok("doc_open", json!({"path": "a.png"})).await;
    let call = |id: i64| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "render_preview", "arguments": {}}}).to_string()
    };
    client.send_raw(&call(900)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    client.send_raw(&call(900)).await;
    let reply = client.next_message().await;
    assert_eq!(
        (reply["error"]["code"].as_i64(), reply["id"].as_i64()),
        (Some(-32600), Some(900)),
        "{reply}"
    );
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("in flight")
    );
    // the original is still running and can be cancelled; its id is then free
    client
        .notify("notifications/cancelled", json!({"requestId": 900}))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let id = client
        .send_request(
            "tools/call",
            json!({"name": "doc_inspect", "arguments": {}}),
        )
        .await;
    assert!(client.reply(id).await.get("error").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_progress_token_is_refused() {
    let (mock, _) = Mock::new();
    let mut client = start(mock, &[], &[]).await;
    let long = "t".repeat(300);
    let reply = client
        .request(
            "tools/call",
            json!({"name": "doc_inspect", "arguments": {}, "_meta": {"progressToken": long}}),
        )
        .await;
    assert_eq!(reply["error"]["code"], json!(-32602), "{reply}");
    let ok = client
        .request(
            "tools/call",
            json!({"name": "doc_inspect", "arguments": {}, "_meta": {"progressToken": "fine"}}),
        )
        .await;
    assert!(ok.get("error").is_none(), "{ok}");
}

// ----- roots ------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_blocking() {
    let d = tempfile::tempdir().unwrap();
    let fifo = d.path().join("pipe.png");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let roots = Roots::open(&[d.path().to_path_buf()], &[]).unwrap();
    let started = Instant::now();
    assert_eq!(
        roots.open_read("pipe.png", 100).unwrap_err(),
        PathError::NotAFile
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn a_dropped_write_target_removes_only_the_file_it_created() {
    let d = tempfile::tempdir().unwrap();
    let roots = Roots::open(&[], &[d.path().to_path_buf()]).unwrap();
    let target = roots.create_write("mine.bin").unwrap();
    // someone replaces the name with a different file meanwhile
    std::fs::remove_file(d.path().join("mine.bin")).unwrap();
    std::fs::write(d.path().join("mine.bin"), b"SOMEONE ELSES").unwrap();
    drop(target);
    assert_eq!(
        std::fs::read(d.path().join("mine.bin")).unwrap(),
        b"SOMEONE ELSES"
    );
    // and the ordinary case still cleans up
    let target = roots.create_write("plain.bin").unwrap();
    drop(target);
    assert!(!d.path().join("plain.bin").exists());
}
