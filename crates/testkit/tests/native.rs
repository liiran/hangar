//! Multi-session HTTP servers (`share = "native"`): one process, and every
//! agent session gets its own MCP HTTP session with its own state.

mod common;

use common::*;
use hangar::client::McpClient;
use hangar::config::{Config, ServerOverride};
use hangar::jsonrpc;
use hangar::policy::Share;
use serde_json::{Value, json};
use std::time::Duration;

fn native_config() -> Config {
    let mut c = Config::default();
    c.servers.insert("web".into(), ServerOverride { share: Some(Share::Native), http_args: Some(vec!["--port".into(), "{port}".into()]) });
    c
}

fn http_cmd(home: &std::path::Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(shim());
    c.arg("--name").arg("web").arg("--").arg(env!("CARGO_BIN_EXE_fake-mcp-http"));
    c.env(hangar::proto::ENV_HOME, home).env(hangar::proto::ENV_NO_AUTOSTART, "1").stderr(std::process::Stdio::null());
    c
}

async fn web_client(home: &std::path::Path) -> McpClient {
    let c = McpClient::spawn(http_cmd(home), vec![]).unwrap();
    let init = c.initialize("t", T).await.unwrap();
    assert_eq!(init["serverInfo"]["name"], "fake-mcp-http");
    c
}

async fn stats(c: &McpClient) -> Value {
    serde_json::from_str(&tool(c, "stats", json!({})).await.unwrap()).unwrap()
}

#[tokio::test]
async fn sessions_share_one_process_but_keep_their_own_state() {
    let home = temp_home("native-share");
    let d = start_daemon(&home, native_config()).await;
    let a = web_client(&home).await;
    let b = web_client(&home).await;
    assert_eq!(tool(&a, "pid", json!({})).await.unwrap(), tool(&b, "pid", json!({})).await.unwrap(), "one server process");
    assert_eq!(tool(&a, "counter", json!({})).await.unwrap(), "1");
    assert_eq!(tool(&a, "counter", json!({})).await.unwrap(), "2");
    assert_eq!(tool(&b, "counter", json!({})).await.unwrap(), "1", "b has its own state");
    assert_eq!(stats(&a).await["sessions"], 2);
    let s = d.pool.stats().await;
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].share, Share::Native);
    assert_eq!(s[0].sessions.len(), 2);
    d.shutdown().await;
}

#[tokio::test]
async fn streamed_progress_and_results_reach_only_their_session() {
    let home = temp_home("native-progress");
    let d = start_daemon(&home, native_config()).await;
    let a = std::sync::Arc::new(web_client(&home).await);
    let b = std::sync::Arc::new(web_client(&home).await);
    let call = |c: std::sync::Arc<McpClient>| async move {
        c.call("tools/call", json!({"name": "slow", "arguments": {}, "_meta": {"progressToken": "p"}}), T).await.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (ra, rb) = tokio::join!(call(a.clone()), call(b.clone()));
    assert_ne!(ra, rb, "each session got its own answer");
    for (c, result) in [(&a, ra), (&b, rb)] {
        let sid = result.trim_start_matches("done ").to_string();
        let mut seen = 0;
        while let Some(n) = c.next_notification(Duration::from_millis(300)).await {
            if n["method"] == "notifications/progress" {
                assert_eq!(n["params"]["message"], sid.as_str(), "progress from another session");
                seen += 1;
            }
        }
        assert_eq!(seen, 3);
    }
    d.shutdown().await;
}

#[tokio::test]
async fn a_leaving_session_closes_its_http_session() {
    let home = temp_home("native-delete");
    let d = start_daemon(&home, native_config()).await;
    let a = web_client(&home).await;
    let b = web_client(&home).await;
    assert_eq!(stats(&b).await["sessions"], 2);
    a.close_stdin().await;
    let mut deleted = 0;
    for _ in 0..50 {
        deleted = stats(&b).await["deleted"].as_array().map(Vec::len).unwrap_or(0);
        if deleted == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(deleted, 1, "the server was told the session ended");
    assert_eq!(stats(&b).await["sessions"], 1);
    d.shutdown().await;
}

#[tokio::test]
async fn sessions_reconnect_by_themselves_after_the_server_restarts() {
    let home = temp_home("native-restart");
    let d = start_daemon(&home, native_config()).await;
    let a = web_client(&home).await;
    let b = web_client(&home).await;
    let before = tool(&a, "pid", json!({})).await.unwrap();
    let crashed = a.request("tools/call", json!({"name": "crash", "arguments": {}}), T).await.unwrap();
    assert_eq!(crashed["error"]["code"], jsonrpc::SERVER_UNAVAILABLE);
    assert!(b.wait_notification("notifications/tools/list_changed", T).await.is_some(), "b told to refresh");
    // Both sessions keep working on the new process without a new initialize.
    let mut after = None;
    for _ in 0..50 {
        if let Ok(p) = tool(&a, "pid", json!({})).await {
            after = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_ne!(after.as_deref(), Some(before.as_str()));
    assert_eq!(tool(&b, "counter", json!({})).await.unwrap(), "1", "fresh state after a restart");
    assert_eq!(stats(&a).await["init_calls"], 2, "hangar re-initialized both sessions");
    assert_eq!(d.pool.stats().await[0].restarts, 1);
    d.shutdown().await;
}
