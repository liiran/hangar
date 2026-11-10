#![allow(dead_code)]
//! Shared helpers for hangar's integration tests.

use anyhow::Result;
use hangar::client::McpClient;
use hangar::config::{Config, ServerOverride};
use hangar::daemon::{Daemon, DaemonOptions};
use hangar::policy::Share;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::process::Command;

pub const T: Duration = Duration::from_secs(20);

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty state directory.
pub fn temp_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hangar-test-{}-{}-{}", tag, std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A config where the named fake servers have the given share levels.
pub fn config(shares: &[(&str, Share)]) -> Config {
    let mut c = Config::default();
    for (name, share) in shares {
        c.servers.insert(name.to_string(), ServerOverride { share: Some(*share) });
    }
    c
}

pub async fn start_daemon(home: &Path, config: Config) -> Daemon {
    Daemon::start(DaemonOptions { home: home.to_path_buf(), config }).await.expect("daemon starts")
}

pub fn fake_mcp() -> &'static str {
    env!("CARGO_BIN_EXE_fake-mcp")
}

pub fn shim() -> &'static str {
    env!("CARGO_BIN_EXE_shim")
}

/// The command an agent would run: the shim in front of the fake server.
pub fn shim_cmd(home: &Path, name: &str, server_args: &[&str]) -> Command {
    let mut c = Command::new(shim());
    c.arg("--name").arg(name).arg("--").arg(fake_mcp()).args(server_args);
    c.env(hangar::proto::ENV_HOME, home).env(hangar::proto::ENV_NO_AUTOSTART, "1");
    c.stderr(std::process::Stdio::null());
    c
}

pub async fn client(home: &Path, name: &str, server_args: &[&str]) -> McpClient {
    let c = McpClient::spawn(shim_cmd(home, name, server_args), vec!["C:/work".into()]).expect("spawn shim");
    c.initialize("test", T).await.expect("initialize");
    c
}

/// Calls a fake-server tool and returns its text output.
pub async fn tool(c: &McpClient, name: &str, args: Value) -> Result<String> {
    let r = c.call("tools/call", json!({"name": name, "arguments": args}), T).await?;
    Ok(r["content"][0]["text"].as_str().unwrap_or_default().to_string())
}

pub async fn server_stats(c: &McpClient) -> Value {
    serde_json::from_str(&tool(c, "stats", json!({})).await.unwrap()).unwrap()
}

/// Polls until `f` is true or the timeout passes.
pub async fn eventually<F: FnMut() -> bool>(timeout: Duration, mut f: F) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}
