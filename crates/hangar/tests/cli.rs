//! The `hangar` command line, run as a real process against temporary
//! state and a temporary `~/.claude.json` (never the user's own).

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hangar-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs `hangar` with an isolated state dir and user profile.
fn hangar(home: &Path, profile: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hangar"))
        .args(args)
        .env("HANGAR_HOME", home)
        .env("USERPROFILE", profile)
        .env("HOME", profile)
        .output()
        .expect("run hangar")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn shim_available() -> bool {
    Path::new(env!("CARGO_BIN_EXE_hangar")).with_file_name(if cfg!(windows) { "hangar-shim.exe" } else { "hangar-shim" }).is_file()
}

#[test]
fn status_and_stop_without_a_daemon_explain_themselves() {
    let (home, profile) = (temp("nodaemon"), temp("nodaemon-p"));
    let o = hangar(&home, &profile, &["status"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("not running"), "{}", stdout(&o));
    assert!(!hangar(&home, &profile, &["stop"]).status.success(), "stop fails when nothing runs");
}

#[test]
fn daemon_starts_reports_status_and_stops() {
    let (home, profile) = (temp("daemon"), temp("daemon-p"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_hangar"))
        .args(["daemon", "--quiet", "--no-ui"])
        .env("HANGAR_HOME", &home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !home.join("daemon.json").exists() {
        assert!(Instant::now() < deadline, "daemon never announced itself");
        std::thread::sleep(Duration::from_millis(50));
    }
    let o = hangar(&home, &profile, &["status", "--json"]);
    let v: Value = serde_json::from_slice(&o.stdout).expect("status --json is JSON");
    assert_eq!(v["ok"], true);
    assert!(v["daemon"].get("token").is_none(), "the token never leaves daemon.json");
    // A second daemon on the same home refuses to start and exits.
    let second = hangar(&home, &profile, &["daemon", "--no-ui"]);
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));
    assert!(hangar(&home, &profile, &["stop"]).status.success());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        assert!(Instant::now() < deadline, "daemon did not stop");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!home.join("daemon.json").exists(), "discovery file removed on exit");
}

#[test]
fn wrap_and_unwrap_round_trip_the_claude_config() {
    if !shim_available() {
        eprintln!("skipped: build the workspace first (cargo build --workspace)");
        return;
    }
    let (home, profile) = (temp("wrap"), temp("wrap-p"));
    let original = json!({
        "numStartups": 3,
        "mcpServers": {
            "memory": {"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"], "env": {}},
            "remote": {"type": "http", "url": "https://example.com/mcp"}
        }
    });
    let path = profile.join(".claude.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();

    let dry = hangar(&home, &profile, &["wrap", "--dry-run"]);
    assert!(stdout(&dry).contains("would change memory"), "{}", stdout(&dry));
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(), original, "dry run writes nothing");

    assert!(hangar(&home, &profile, &["wrap"]).status.success());
    let wrapped: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(wrapped["mcpServers"]["memory"]["command"].as_str().unwrap().contains("hangar-shim"));
    assert_eq!(wrapped["mcpServers"]["remote"], original["mcpServers"]["remote"], "remote servers untouched");
    assert_eq!(wrapped["numStartups"], 3, "the rest of the file is kept");
    assert_eq!(std::fs::read_dir(home.join("backups")).unwrap().count(), 1, "a backup was written");
    assert!(stdout(&hangar(&home, &profile, &["wrap"])).contains("no stdio MCP servers to wrap"), "wrapping twice is a no-op");

    assert!(hangar(&home, &profile, &["unwrap"]).status.success());
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(), original, "unwrap restores the original");
}

#[test]
fn wrap_refuses_a_broken_config_instead_of_overwriting_it() {
    if !shim_available() {
        return;
    }
    let (home, profile) = (temp("broken"), temp("broken-p"));
    let path = profile.join(".claude.json");
    std::fs::write(&path, "{ not json").unwrap();
    let o = hangar(&home, &profile, &["wrap"]);
    assert!(!o.status.success());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}
