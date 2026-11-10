//! The npx fast path: a cached package is started with `node` directly, and
//! a package that fails that way falls back to the configured npx command.
//! Uses a fake npm cache and a fake `npx` on PATH, so no network is needed.

mod common;

use common::*;
use hangar::client::McpClient;
use hangar::policy::Share;
use serde_json::json;
use std::path::{Path, PathBuf};

/// An exact version: npx never updates it, so the fast path applies at once.
const PKG: &str = "hangar-fake-server@1.0.0";
const LATEST: &str = "hangar-fake-server@latest";
const FORWARD_JS: &str =
    "const c = require('child_process').spawn(process.env.FAKE_MCP, [], { stdio: 'inherit' }); c.on('exit', (code) => process.exit(code ?? 0));";

/// Builds a fake npm cache holding PKG with the given entry script, and a
/// fake `npx` that runs the fake MCP server. Returns (cache, bin dir).
fn fixture(tag: &str, entry_js: &str, spec: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = temp_home(tag);
    let cache = root.join("cache");
    let pkg = hangar::launch::npx_dir(&cache, spec).join("node_modules").join("hangar-fake-server");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("package.json"), r#"{"name":"hangar-fake-server","bin":"server.js"}"#).unwrap();
    std::fs::write(pkg.join("server.js"), entry_js).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    if cfg!(windows) {
        std::fs::write(bin.join("npx.cmd"), "@echo off\r\n\"%FAKE_MCP%\" %*\r\n").unwrap();
    } else {
        let p = bin.join("npx");
        std::fs::write(&p, "#!/bin/sh\nexec \"$FAKE_MCP\" \"$@\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    (root, cache, bin)
}

fn client_cmd(home: &Path, cache: &Path, bin: &Path, spec: &str) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(shim());
    c.args(["--name", "fp", "--", "npx", "-y", spec]);
    let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    c.env(hangar::proto::ENV_HOME, home)
        .env(hangar::proto::ENV_NO_AUTOSTART, "1")
        .env("PATH", path)
        .env("npm_config_cache", cache)
        .env("FAKE_MCP", fake_mcp())
        .stderr(std::process::Stdio::null());
    c
}

fn node_available() -> bool {
    hangar_shim::resolve_command("node").is_some()
}

#[tokio::test]
async fn a_cached_package_starts_without_npx() {
    if !node_available() {
        eprintln!("skipped: node is not installed");
        return;
    }
    let (root, cache, bin) = fixture("fast-ok", FORWARD_JS, PKG);
    let d = start_daemon(&root, config(&[("fp", Share::Global)])).await;
    let c = McpClient::spawn(client_cmd(&root, &cache, &bin, PKG), vec![]).unwrap();
    c.initialize("t", T).await.unwrap();
    assert_eq!(tool(&c, "echo", json!({"text": "fast"})).await.unwrap(), "fast");
    let s = &d.pool.stats().await[0];
    assert!(s.fast_path, "started through the fast path");
    assert!(s.launch.as_deref().unwrap().contains("server.js"), "{:?}", s.launch);
    d.shutdown().await;
}

#[tokio::test]
async fn a_broken_fast_start_falls_back_to_npx() {
    if !node_available() {
        return;
    }
    let (root, cache, bin) = fixture("fast-fallback", "process.exit(1);", PKG);
    let d = start_daemon(&root, config(&[("fp", Share::Global)])).await;
    let c = McpClient::spawn(client_cmd(&root, &cache, &bin, PKG), vec![]).unwrap();
    c.initialize("t", T).await.expect("the client never sees the failed fast start");
    assert_eq!(tool(&c, "echo", json!({"text": "slow but fine"})).await.unwrap(), "slow but fine");
    let s = &d.pool.stats().await[0];
    assert!(!s.fast_path, "fell back to the configured command");
    assert!(s.launch.as_deref().unwrap().to_ascii_lowercase().contains("npx"), "{:?}", s.launch);
    d.shutdown().await;
}

#[tokio::test]
async fn servers_resolve_commands_on_the_clients_path() {
    // `npx` here exists only on the PATH the client passed, not the daemon's.
    let (root, cache, bin) = fixture("client-path", "process.exit(1);", PKG);
    let mut cfg = config(&[("fp", Share::Global)]);
    cfg.pool.fast_npx = false;
    let d = start_daemon(&root, cfg).await;
    let c = McpClient::spawn(client_cmd(&root, &cache, &bin, PKG), vec![]).unwrap();
    c.initialize("t", T).await.unwrap();
    assert_eq!(tool(&c, "echo", json!({"text": "found"})).await.unwrap(), "found");
    d.shutdown().await;
}

#[tokio::test]
async fn tagged_versions_go_through_npx_until_it_has_refreshed_the_cache() {
    if !node_available() {
        return;
    }
    let (root, cache, bin) = fixture("fast-latest", FORWARD_JS, LATEST);
    let d = start_daemon(&root, config(&[("fp", Share::Session)])).await;
    // First start: npx itself runs (it may update @latest), and hangar notes it.
    let a = McpClient::spawn(client_cmd(&root, &cache, &bin, LATEST), vec![]).unwrap();
    a.initialize("t", T).await.unwrap();
    assert!(!d.pool.stats().await[0].fast_path, "@latest first goes through npx");
    // A second session within the freshness window starts fast.
    let b = McpClient::spawn(client_cmd(&root, &cache, &bin, LATEST), vec![]).unwrap();
    b.initialize("t", T).await.unwrap();
    let fast: Vec<bool> = d.pool.stats().await.iter().map(|s| s.fast_path).collect();
    assert_eq!(fast.iter().filter(|f| **f).count(), 1, "{fast:?}");
    assert_eq!(tool(&b, "echo", json!({"text": "ok"})).await.unwrap(), "ok");
    d.shutdown().await;
}
