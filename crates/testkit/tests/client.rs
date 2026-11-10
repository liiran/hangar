//! The MCP client the benchmark, verifier and tests use to stand in for an
//! agent: it must fail fast and tolerate messy servers, or every measurement
//! built on it is wrong.

mod common;

use common::*;
use hangar::client::McpClient;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::process::Command;

fn direct(args: &[&str]) -> McpClient {
    let mut cmd = Command::new(fake_mcp());
    cmd.args(args).stderr(std::process::Stdio::null());
    McpClient::spawn(cmd, vec!["C:/work".into()]).expect("spawn fake-mcp")
}

#[tokio::test]
async fn requests_fail_at_once_after_the_server_exits() {
    let c = direct(&[]);
    c.initialize("test", T).await.unwrap();
    // The server exits instead of answering.
    let started = Instant::now();
    assert!(c.call("tools/call", json!({"name": "crash", "arguments": {}}), Duration::from_secs(30)).await.is_err());
    assert!(started.elapsed() < Duration::from_secs(5), "the waiting request failed when stdout closed");
    // Later requests do not wait for their timeout either.
    let started = Instant::now();
    let err = c.call("tools/list", json!({}), Duration::from_secs(30)).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}: {err:#}", started.elapsed());
}

#[tokio::test]
async fn timed_out_requests_do_not_linger() {
    let c = direct(&[]);
    c.initialize("test", T).await.unwrap();
    let err = c.call("tools/call", json!({"name": "slow", "arguments": {"ms": 3000}}), Duration::from_millis(200)).await.unwrap_err();
    assert!(format!("{err:#}").contains("timed out"), "{err:#}");
    // The late answer to the abandoned request is ignored, and the next
    // request gets its own answer.
    let text = tool(&c, "echo", json!({"text": "after"})).await.unwrap();
    assert_eq!(text, "after");
}

#[tokio::test]
async fn junk_on_stdout_is_skipped_direct_and_pooled() {
    let c = direct(&["--stdout-junk"]);
    c.initialize("test", T).await.unwrap();
    assert!(c.call("tools/list", json!({}), T).await.unwrap()["tools"].as_array().is_some_and(|t| !t.is_empty()));

    let home = temp_home("client-junk");
    let daemon = start_daemon(&home, config(&[])).await;
    let pooled = client(&home, "junk", &["--stdout-junk"]).await;
    assert!(pooled.call("tools/list", json!({}), T).await.unwrap()["tools"].as_array().is_some_and(|t| !t.is_empty()));
    assert_eq!(tool(&pooled, "echo", json!({"text": "still fine"})).await.unwrap(), "still fine");
    daemon.shutdown().await;
}
