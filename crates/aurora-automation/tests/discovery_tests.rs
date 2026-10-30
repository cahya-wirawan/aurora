//! Discovery file: permissions, no replacement, stale sweep, foreign files.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use std::sync::Arc;
use std::time::Duration;

use aurora_automation::bridge::client::BridgeClient;
use aurora_automation::bridge::discovery::{
    Discovery, ensure_private_dir, find_running, publish, read_file, read_live, rotate_token,
    sweep_stale,
};
use aurora_automation::bridge::protocol::{CallError, Request};
use aurora_automation::bridge::server::{BridgeServer, Dispatcher, ServerConfig};
use aurora_automation::bridge::token;
use serde_json::{Value, json};

fn record(pid: u32) -> Discovery {
    let mut d = Discovery::new(4242, token::generate().unwrap(), "0.0.0");
    d.pid = pid;
    d
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().mode() & 0o777
}

#[test]
fn files_are_owner_only_whatever_the_umask_and_the_dir_is_0700() {
    // A permissive umask must not make the files or directory readable.
    let previous = rustix::process::umask(rustix::fs::Mode::empty());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bridge");
    let rec = Discovery::new(4242, token::generate().unwrap(), "0.0.0");
    let published = publish(&dir, &rec).unwrap();
    let json_mode = mode(published.path());
    let dir_mode = mode(&dir);
    let lock_mode = mode(&dir.join(format!("aurora-bridge-{}.lock", rec.pid)));
    rustix::process::umask(previous);
    assert_eq!(json_mode, 0o600, "discovery file mode");
    assert_eq!(lock_mode, 0o600, "lock file mode");
    assert_eq!(dir_mode, 0o700, "directory mode");
    assert_eq!(read_file(published.path()).unwrap(), rec);
}

#[test]
fn a_loose_existing_directory_is_tightened_and_a_symlink_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let loose = tmp.path().join("loose");
    std::fs::create_dir(&loose).unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o777)).unwrap();
    ensure_private_dir(&loose).unwrap();
    assert_eq!(mode(&loose), 0o700);
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&loose, &link).unwrap();
    assert!(ensure_private_dir(&link).is_err());
}

#[test]
fn a_readable_discovery_file_is_not_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let published = publish(&dir, &record(std::process::id())).unwrap();
    std::fs::set_permissions(published.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = read_file(published.path()).unwrap_err().to_string();
    assert!(err.contains("readable by other users"), "{err}");
}

#[test]
fn a_second_publish_never_replaces_a_live_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let first = publish(&dir, &record(std::process::id())).unwrap();
    let before = std::fs::read_to_string(first.path()).unwrap();
    assert!(publish(&dir, &record(std::process::id())).is_err());
    assert_eq!(std::fs::read_to_string(first.path()).unwrap(), before);
}

#[test]
fn drop_removes_the_files_and_find_running_sees_a_live_app() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let rec = record(std::process::id());
    let published = publish(&dir, &rec).unwrap();
    let (path, found) = find_running(&dir).unwrap();
    assert_eq!(path, published.path());
    assert_eq!(found, rec);
    drop(published);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    assert!(find_running(&dir).is_err());
}

fn crashed(dir: &Path, pid: u32) {
    // What a killed app leaves: the json and an unlocked lock file.
    ensure_private_dir(dir).unwrap();
    let json = dir.join(format!("aurora-bridge-{pid}.json"));
    std::fs::write(&json, serde_json::to_string(&record(pid)).unwrap()).unwrap();
    std::fs::set_permissions(&json, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(dir.join(format!("aurora-bridge-{pid}.lock")), b"").unwrap();
}

#[test]
fn the_sweep_removes_a_crashed_apps_files_only() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    crashed(&dir, 999_001);
    // Foreign material that must survive.
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    std::fs::write(dir.join("aurora-bridge-notes.json"), b"{}").unwrap();
    std::fs::write(dir.join("aurora-bridge-12.json"), b"{\"x\":1}").unwrap();
    std::fs::write(dir.join("aurora-bridge-007.json"), b"{}").unwrap();
    std::fs::create_dir(dir.join("aurora-bridge-5.json")).unwrap();
    let outside = tmp.path().join("target.json");
    std::fs::write(&outside, serde_json::to_string(&record(77)).unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("aurora-bridge-77.json")).unwrap();
    let report = sweep_stale(&dir);
    assert_eq!(report.removed, 1);
    assert!(!dir.join("aurora-bridge-999001.json").exists());
    assert!(!dir.join("aurora-bridge-999001.lock").exists());
    for kept in [
        "notes.txt",
        "aurora-bridge-notes.json",
        "aurora-bridge-12.json",
        "aurora-bridge-007.json",
        "aurora-bridge-5.json",
        "aurora-bridge-77.json",
    ] {
        assert!(
            std::fs::symlink_metadata(dir.join(kept)).is_ok(),
            "{kept} must survive"
        );
    }
    assert!(outside.exists(), "a symlink target is never touched");
}

#[test]
fn the_sweep_leaves_a_live_apps_files_and_a_json_without_a_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let live = publish(&dir, &record(std::process::id())).unwrap();
    // json with no lock file at all: could be a live app whose lock a cleaner removed
    let orphan = dir.join("aurora-bridge-999002.json");
    std::fs::write(&orphan, serde_json::to_string(&record(999_002)).unwrap()).unwrap();
    std::fs::set_permissions(&orphan, std::fs::Permissions::from_mode(0o600)).unwrap();
    let report = sweep_stale(&dir);
    assert_eq!(report.removed, 0);
    assert!(live.path().exists());
    assert!(orphan.exists());
}

#[test]
fn a_file_naming_another_pid_is_not_swept() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    crashed(&dir, 999_003);
    let json = dir.join("aurora-bridge-999003.json");
    std::fs::write(&json, serde_json::to_string(&record(1)).unwrap()).unwrap();
    std::fs::set_permissions(&json, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(sweep_stale(&dir).removed, 0);
    assert!(json.exists());
}

#[test]
fn a_sweeper_never_unlinks_a_lock_file_a_publisher_is_taking() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    ensure_private_dir(&dir).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let sweepers: Vec<_> = (0..3)
        .map(|_| {
            let (dir, stop) = (dir.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _ = sweep_stale(&dir);
                }
            })
        })
        .collect();
    for round in 0..300 {
        let published =
            publish(&dir, &Discovery::new(4242, token::generate().unwrap(), "t")).unwrap();
        // The lock we hold must still be the file the name points at, so
        // a proxy sees this app as alive.
        let (path, _) = find_running(&dir).expect("the published app is found");
        let _ = round;
        assert_eq!(path, published.path());
        drop(published);
    }
    stop.store(true, Ordering::SeqCst);
    for s in sweepers {
        s.join().unwrap();
    }
}

#[test]
fn an_orphan_lock_file_is_removed_only_when_nobody_holds_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    ensure_private_dir(&dir).unwrap();
    let lock = dir.join("aurora-bridge-999004.lock");
    std::fs::write(&lock, b"").unwrap();
    let held = std::fs::OpenOptions::new().read(true).open(&lock).unwrap();
    held.try_lock().unwrap();
    assert_eq!(sweep_stale(&dir).removed, 0, "a held lock is not an orphan");
    assert!(lock.exists());
    drop(held);
    assert_eq!(sweep_stale(&dir).removed, 1);
    assert!(!lock.exists());
}

// ---- task 165: liveness of an explicitly named file ----

#[test]
fn a_named_discovery_file_must_belong_to_a_live_app() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let live = publish(&dir, &record(std::process::id())).unwrap();
    assert_eq!(
        read_live(live.path()).unwrap().pid,
        std::process::id(),
        "a held lock is a live app"
    );
    let path = live.path().to_path_buf();
    drop(live);
    // The app is gone (its lock is released). Put its file back, as a crash
    // or a SIGTERM leaves it.
    crashed(&dir, std::process::id());
    let stale = read_live(&path).unwrap_err().to_string();
    assert!(
        stale.contains("stale") && stale.contains("not sent"),
        "{stale}"
    );
    // read_file alone would have trusted it: that is the old behaviour.
    assert!(read_file(&path).is_ok());
}

#[test]
fn a_discovery_file_with_no_lock_beside_it_is_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    crashed(&dir, 999_010);
    std::fs::remove_file(dir.join("aurora-bridge-999010.lock")).unwrap();
    // Renamed to something the sweep would never touch.
    let json = dir.join("mine.json");
    std::fs::rename(dir.join("aurora-bridge-999010.json"), &json).unwrap();
    assert!(read_live(&json).unwrap_err().to_string().contains("stale"));
}

// ---- task 165: token rotation ----

struct Quiet;

impl Dispatcher for Quiet {
    fn dispatch(&self, request: Request) -> Result<Value, CallError> {
        Ok(json!({"echo": request.method}))
    }
}

fn server_for(token: &str) -> BridgeServer {
    BridgeServer::start(
        ServerConfig {
            fail_delay: Duration::from_millis(20),
            ..ServerConfig::default()
        },
        token.to_owned(),
        Arc::new(Quiet),
    )
    .unwrap()
}

fn connect(server: &BridgeServer, token: &str) -> Result<BridgeClient, String> {
    BridgeClient::connect_to(server.local_addr(), token, Duration::from_secs(3))
        .map_err(|e| e.to_string())
}

#[test]
fn rotating_the_token_replaces_the_file_and_refuses_the_old_token() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let first = token::generate().unwrap();
    let server = server_for(&first);
    let mut record = Discovery::new(server.local_addr().port(), first.clone(), "t");
    let mut published = publish(&dir, &record).unwrap();

    // A controller works, leaves; the token is rotated.
    let mut client = connect(&server, &first).unwrap();
    client.call("x", json!({}), Duration::from_secs(3)).unwrap();
    drop(client);
    rotate_token(&server, &mut published, &mut record).unwrap();

    assert_ne!(record.token, first);
    let on_disk = read_live(published.path()).unwrap();
    assert_eq!(on_disk, record, "the file holds the new token");
    assert_eq!(mode(published.path()), 0o600);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(!names.iter().any(|n| n.contains(".tmp")), "{names:?}");

    // The old token no longer opens the door; the new one does.
    let refused = connect(&server, &first).expect_err("old token refused");
    assert!(refused.contains("authentication failed"), "{refused}");
    let mut again = connect(&server, &on_disk.token).unwrap();
    again.call("x", json!({}), Duration::from_secs(3)).unwrap();
}

#[test]
fn a_rotation_leaves_a_connected_controller_connected() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let first = token::generate().unwrap();
    let server = server_for(&first);
    let mut record = Discovery::new(server.local_addr().port(), first.clone(), "t");
    let mut published = publish(&dir, &record).unwrap();
    let mut client = connect(&server, &first).unwrap();
    rotate_token(&server, &mut published, &mut record).unwrap();
    client
        .call("x", json!({}), Duration::from_secs(3))
        .expect("the session that was authenticated keeps working");
}

#[test]
fn a_failed_rewrite_keeps_the_old_file_and_the_old_token() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    let first = token::generate().unwrap();
    let server = server_for(&first);
    let mut record = Discovery::new(server.local_addr().port(), first.clone(), "t");
    let mut published = publish(&dir, &record).unwrap();
    let before = std::fs::read_to_string(published.path()).unwrap();
    // A record for another process is refused by the rewrite.
    let mut stranger = record.clone();
    stranger.pid = record.pid.wrapping_add(1);
    stranger.token = token::generate().unwrap();
    assert!(published.rewrite(&stranger).is_err());
    assert_eq!(std::fs::read_to_string(published.path()).unwrap(), before);
    connect(&server, &first).expect("the old token still works");
    // A malformed token is refused by the server and by the rewrite.
    assert!(server.set_token("not a token").is_err());
    record.token = "zz".into();
    assert!(published.rewrite(&record).is_err());
}

#[test]
fn the_sweep_removes_a_dead_rotations_temp_file_and_spares_a_live_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("b");
    // (publish sweeps too, so the dead app's files are made after it.)
    let live = publish(&dir, &record(std::process::id())).unwrap();
    crashed(&dir, 999_020);
    let dead = dir.join("aurora-bridge-999020.json.tmp");
    std::fs::write(&dead, b"{}").unwrap();
    let alive = dir.join(format!("aurora-bridge-{}.json.tmp", std::process::id()));
    std::fs::write(&alive, b"{}").unwrap();
    let report = sweep_stale(&dir);
    assert_eq!(report.removed, 2, "the crashed app's json and its temp");
    assert!(!dead.exists());
    assert!(
        alive.exists(),
        "a held lock means a rotation may be running"
    );
    drop(live);
}
