//! End-to-end tests of the pool: real shim processes, a real daemon, and a
//! fake MCP server whose behavior the tests control.

mod common;

use common::*;
use hangar::client::McpClient;
use hangar::jsonrpc;
use hangar::policy::Share;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;

#[tokio::test]
async fn shared_server_runs_once_for_many_sessions() {
    let home = temp_home("shared");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = client(&home, "fake", &[]).await;
    let c = client(&home, "fake", &[]).await;
    let pa = tool(&a, "pid", json!({})).await.unwrap();
    assert_eq!(pa, tool(&b, "pid", json!({})).await.unwrap());
    assert_eq!(pa, tool(&c, "pid", json!({})).await.unwrap());
    // The process-wide counter proves it is literally one process.
    tool(&a, "counter", json!({})).await.unwrap();
    assert_eq!(tool(&b, "counter", json!({})).await.unwrap(), "2");
    let stats = d.pool.stats().await;
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].sessions.len(), 3);
    // The server was initialized once, and never saw client capabilities.
    let s = server_stats(&a).await;
    assert_eq!(s["init_calls"], 1);
    assert_eq!(s["client_capabilities"], json!({}));
    d.shutdown().await;
}

#[tokio::test]
async fn session_share_gives_each_session_its_own_process() {
    let home = temp_home("session");
    let d = start_daemon(&home, config(&[("fake", Share::Session)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = client(&home, "fake", &[]).await;
    assert_ne!(tool(&a, "pid", json!({})).await.unwrap(), tool(&b, "pid", json!({})).await.unwrap());
    // A per-session server sees the real client capabilities.
    assert_eq!(server_stats(&a).await["client_capabilities"]["roots"]["listChanged"], true);
    assert_eq!(d.pool.stats().await.len(), 2);
    d.shutdown().await;
}

#[tokio::test]
async fn project_share_splits_by_folder() {
    let home = temp_home("project");
    let d = start_daemon(&home, config(&[("fake", Share::Project)])).await;
    let dir1 = temp_home("p1");
    let dir2 = temp_home("p2");
    let spawn_in = |dir: &std::path::Path| {
        let mut cmd = shim_cmd(&home, "fake", &[]);
        cmd.current_dir(dir);
        McpClient::spawn(cmd, vec![]).unwrap()
    };
    let a = spawn_in(&dir1);
    let b = spawn_in(&dir1);
    let c = spawn_in(&dir2);
    for x in [&a, &b, &c] {
        x.initialize("t", T).await.unwrap();
    }
    let (pa, pb, pc) = (tool(&a, "pid", json!({})).await.unwrap(), tool(&b, "pid", json!({})).await.unwrap(), tool(&c, "pid", json!({})).await.unwrap());
    assert_eq!(pa, pb);
    assert_ne!(pa, pc);
    d.shutdown().await;
}

#[tokio::test]
async fn overlapping_request_ids_are_routed_to_the_right_session() {
    let home = temp_home("ids");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = std::sync::Arc::new(client(&home, "fake", &[]).await);
    let b = std::sync::Arc::new(client(&home, "fake", &[]).await);
    let mut tasks = Vec::new();
    for (tag, c) in [("a", a.clone()), ("b", b.clone())] {
        for i in 0..40u64 {
            let c = c.clone();
            tasks.push(tokio::spawn(async move {
                // Both sessions deliberately use the same ids.
                let id = json!(10_000 + i);
                let text = format!("{tag}-{i}");
                let r = c.request_with_id(id.clone(), "tools/call", json!({"name": "echo", "arguments": {"text": text}}), T).await.unwrap();
                assert_eq!(r["id"], id);
                assert_eq!(r["result"]["content"][0]["text"], text);
            }));
        }
    }
    for t in tasks {
        t.await.unwrap();
    }
    d.shutdown().await;
}

#[tokio::test]
async fn progress_only_reaches_the_session_that_asked() {
    let home = temp_home("progress");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = std::sync::Arc::new(client(&home, "fake", &[]).await);
    let b = std::sync::Arc::new(client(&home, "fake", &[]).await);
    let call = |c: std::sync::Arc<McpClient>, tag: &'static str| async move {
        c.call("tools/call", json!({"name": "slow", "arguments": {"ms": 300, "steps": 3, "tag": tag}, "_meta": {"progressToken": "same-token"}}), T).await.unwrap()
    };
    let (ra, rb) = tokio::join!(call(a.clone(), "A"), call(b.clone(), "B"));
    assert_eq!(ra["content"][0]["text"], "done A");
    assert_eq!(rb["content"][0]["text"], "done B");
    for (c, tag) in [(&a, "A"), (&b, "B")] {
        let mut seen = 0;
        while let Some(n) = c.next_notification(Duration::from_millis(300)).await {
            if n["method"] == "notifications/progress" {
                assert_eq!(n["params"]["progressToken"], "same-token", "token restored");
                assert_eq!(n["params"]["message"], tag, "no cross-session progress");
                seen += 1;
            }
        }
        assert_eq!(seen, 3);
    }
    d.shutdown().await;
}

#[tokio::test]
async fn initialize_and_tool_lists_are_cached_until_changed() {
    let home = temp_home("cache");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = McpClient::spawn(shim_cmd(&home, "fake", &[]), vec![]).unwrap();
    let ib = b.initialize("t", T).await.unwrap();
    assert_eq!(ib["serverInfo"]["name"], "fake-mcp");
    assert_eq!(ib["instructions"], "a fake server");
    let la = a.call("tools/list", json!({}), T).await.unwrap();
    let lb = b.call("tools/list", json!({}), T).await.unwrap();
    assert_eq!(la, lb);
    assert_eq!(server_stats(&a).await["tools_list_calls"], 1, "second list served from cache");
    tool(&a, "notify_changed", json!({})).await.unwrap();
    for c in [&a, &b] {
        assert!(c.wait_notification("notifications/tools/list_changed", T).await.is_some(), "every session hears list_changed");
    }
    b.call("tools/list", json!({}), T).await.unwrap();
    assert_eq!(server_stats(&a).await["tools_list_calls"], 2, "cache invalidated");
    d.shutdown().await;
}

#[tokio::test]
async fn crashed_server_is_restarted_and_sessions_are_told() {
    let home = temp_home("crash");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = client(&home, "fake", &[]).await;
    let before = tool(&a, "pid", json!({})).await.unwrap();
    let crashed = a.request("tools/call", json!({"name": "crash", "arguments": {}}), T).await.unwrap();
    assert_eq!(crashed["error"]["code"], jsonrpc::SERVER_UNAVAILABLE);
    assert!(b.wait_notification("notifications/tools/list_changed", T).await.is_some());
    let after = tool(&b, "pid", json!({})).await.unwrap();
    assert_ne!(before, after, "a new process");
    assert_eq!(tool(&a, "echo", json!({"text": "still here"})).await.unwrap(), "still here");
    let stats = d.pool.stats().await;
    assert_eq!(stats[0].restarts, 1);
    assert_eq!(stats[0].sessions.len(), 2, "nobody was dropped");
    d.shutdown().await;
}

#[tokio::test]
async fn leaving_session_cancels_its_requests_upstream() {
    let home = temp_home("cancel");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = client(&home, "fake", &[]).await;
    a.send(&jsonrpc::request(json!(77), "tools/call", json!({"name": "slow", "arguments": {"ms": 3000}}))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.close_stdin().await;
    let mut cancelled = Value::Null;
    for _ in 0..40 {
        cancelled = server_stats(&b).await["cancelled"].clone();
        if cancelled.as_array().is_some_and(|c| !c.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(cancelled.as_array().map(Vec::len), Some(1), "server told to stop the abandoned call");
    d.shutdown().await;
}

#[tokio::test]
async fn explicit_cancel_is_remapped() {
    let home = temp_home("cancel2");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    a.send(&jsonrpc::request(json!("mine"), "tools/call", json!({"name": "slow", "arguments": {"ms": 1500}}))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.notify("notifications/cancelled", Some(json!({"requestId": "mine", "reason": "user"}))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let cancelled = server_stats(&a).await["cancelled"].clone();
    let ids = cancelled.as_array().unwrap();
    assert_eq!(ids.len(), 1);
    assert!(ids[0].is_u64(), "the server sees its own id space, not the client's: {ids:?}");
    d.shutdown().await;
}

#[tokio::test]
async fn idle_shared_server_is_stopped() {
    let home = temp_home("idle");
    let mut cfg = config(&[("fake", Share::Global)]);
    cfg.pool.idle_timeout_secs = 1;
    let d = start_daemon(&home, cfg).await;
    let a = client(&home, "fake", &[]).await;
    let pid: u32 = tool(&a, "pid", json!({})).await.unwrap().parse().unwrap();
    a.close_stdin().await;
    let pool = d.pool.clone();
    assert!(eventually(Duration::from_secs(10), || !hangar::procs::is_alive(pid)).await, "idle server stopped");
    assert!(pool.stats().await.is_empty());
    // A new session starts it again.
    let b = client(&home, "fake", &[]).await;
    assert_ne!(tool(&b, "pid", json!({})).await.unwrap(), pid.to_string());
    d.shutdown().await;
}

#[tokio::test]
async fn subscriptions_are_reference_counted() {
    let home = temp_home("subs");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = client(&home, "fake", &[]).await;
    let c = client(&home, "fake", &[]).await;
    a.call("resources/subscribe", json!({"uri": "mem://a"}), T).await.unwrap();
    b.call("resources/subscribe", json!({"uri": "mem://a"}), T).await.unwrap();
    assert_eq!(server_stats(&a).await["subscribe_calls"], 1);
    tool(&c, "touch", json!({"uri": "mem://a"})).await.unwrap();
    assert!(a.wait_notification("notifications/resources/updated", T).await.is_some());
    assert!(b.wait_notification("notifications/resources/updated", T).await.is_some());
    assert!(c.wait_notification("notifications/resources/updated", Duration::from_millis(400)).await.is_none(), "non-subscriber not told");
    a.call("resources/unsubscribe", json!({"uri": "mem://a"}), T).await.unwrap();
    assert_eq!(server_stats(&a).await["unsubscribe_calls"], 0);
    b.call("resources/unsubscribe", json!({"uri": "mem://a"}), T).await.unwrap();
    assert_eq!(server_stats(&a).await["unsubscribe_calls"], 1);
    d.shutdown().await;
}

#[tokio::test]
async fn shared_server_cannot_ask_one_session_for_its_roots() {
    let home = temp_home("roots");
    let d = start_daemon(&home, config(&[("shared", Share::Global), ("own", Share::Session)])).await;
    let s = client(&home, "shared", &[]).await;
    let answer = tool(&s, "ask_roots", json!({})).await.unwrap();
    assert!(answer.contains(&jsonrpc::METHOD_NOT_FOUND.to_string()), "refused: {answer}");
    let o = client(&home, "own", &[]).await;
    let answer = tool(&o, "ask_roots", json!({})).await.unwrap();
    assert!(answer.contains("file:///C:/work"), "per-session server gets the client's roots: {answer}");
    d.shutdown().await;
}

#[tokio::test]
async fn shim_runs_the_server_itself_when_no_daemon_is_available() {
    let home = temp_home("fallback");
    let c = client(&home, "fake", &[]).await;
    let pid = tool(&c, "pid", json!({})).await.unwrap();
    assert_ne!(pid, c.pid().to_string(), "the fake server is a child of the shim");
    assert_eq!(tool(&c, "echo", json!({"text": "ok"})).await.unwrap(), "ok");
    c.close_stdin().await;
    let pid: u32 = pid.parse().unwrap();
    assert!(eventually(Duration::from_secs(10), || !hangar::procs::is_alive(pid)).await);
}

#[tokio::test]
async fn shim_reconnects_after_the_daemon_restarts() {
    let home = temp_home("reconnect");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let before = tool(&a, "pid", json!({})).await.unwrap();
    d.shutdown().await;
    let d2 = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    // No new initialize from the client; the shim replays it.
    let mut text = None;
    for _ in 0..50 {
        if let Ok(t) = tool(&a, "echo", json!({"text": "back"})).await {
            text = Some(t);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(text.as_deref(), Some("back"));
    assert_ne!(tool(&a, "pid", json!({})).await.unwrap(), before);
    assert_eq!(server_stats(&a).await["init_calls"], 1);
    d2.shutdown().await;
}

#[tokio::test]
async fn daemon_shutdown_stops_every_server() {
    let home = temp_home("shutdown");
    let d = start_daemon(&home, config(&[("fake", Share::Global), ("own", Share::Session)])).await;
    let a = client(&home, "fake", &[]).await;
    let b = client(&home, "own", &[]).await;
    let pids: Vec<u32> = vec![tool(&a, "pid", json!({})).await.unwrap().parse().unwrap(), tool(&b, "pid", json!({})).await.unwrap().parse().unwrap()];
    d.shutdown().await;
    assert!(eventually(Duration::from_secs(10), || pids.iter().all(|p| !hangar::procs::is_alive(*p))).await);
    assert!(hangar::proto::read_daemon_info(&home).is_none(), "discovery file removed");
}

#[tokio::test]
async fn large_and_unicode_payloads_pass_through_intact() {
    let home = temp_home("payload");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let a = client(&home, "fake", &[]).await;
    let big = tool(&a, "big", json!({"kb": 4096})).await.unwrap();
    assert_eq!(big.len(), 4096 * 1024);
    let text = "héllo 🚀 \u{2028} \"quoted\" \\ back\nslash";
    assert_eq!(tool(&a, "echo", json!({"text": text})).await.unwrap(), text);
    d.shutdown().await;
}

#[tokio::test]
async fn failing_server_reports_an_error_instead_of_hanging() {
    let home = temp_home("failinit");
    let d = start_daemon(&home, config(&[("bad", Share::Global)])).await;
    let c = McpClient::spawn(shim_cmd(&home, "bad", &["--fail-init"]), vec![]).unwrap();
    let r = c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}), T).await.unwrap();
    assert_eq!(r["error"]["code"], jsonrpc::SERVER_UNAVAILABLE);
    assert!(r["error"]["message"].as_str().unwrap().contains("refusing to initialize"));
    d.shutdown().await;
}

#[tokio::test]
async fn status_lists_servers_and_sessions() {
    let home = temp_home("status");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let _a = client(&home, "fake", &[]).await;
    let _b = client(&home, "fake", &[]).await;
    let v = hangar::daemon::send_control(&home, "status", json!({})).await.unwrap();
    assert_eq!(v["ok"], true);
    let servers = v["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["sessions"].as_array().unwrap().len(), 2);
    assert!(servers[0]["memory"]["private_bytes"].as_u64().unwrap() > 0);
    assert!(v["daemon"].get("token").is_none(), "status never leaks the token");
    // A connection without the right token gets nothing.
    let info = hangar::proto::read_daemon_info(&home).unwrap();
    let mut raw = tokio::net::TcpStream::connect(("127.0.0.1", info.shim_port)).await.unwrap();
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    raw.write_all(b"{\"control\":\"status\",\"token\":\"nope\"}
").await.unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(raw).read_line(&mut line).await.unwrap();
    let bad: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(bad["ok"], false);
    assert!(bad.get("servers").is_none());
    let pids: HashSet<u64> = servers.iter().filter_map(|s| s["pid"].as_u64()).collect();
    assert_eq!(pids.len(), 1);
    d.shutdown().await;
}

#[tokio::test]
async fn probes_before_initialize_get_method_not_found() {
    // Claude Code 2.1.3xx sends `server/discover` first and falls back to initialize.
    let home = temp_home("probe");
    let d = start_daemon(&home, config(&[("fake", Share::Global)])).await;
    let c = McpClient::spawn(shim_cmd(&home, "fake", &[]), vec![]).unwrap();
    let probe = c.request_with_id(json!("server-discover-probe-1"), "server/discover", json!({"_meta": {}}), T).await.unwrap();
    assert_eq!(probe["error"]["code"], jsonrpc::METHOD_NOT_FOUND);
    c.initialize("t", T).await.unwrap();
    assert_eq!(tool(&c, "echo", json!({"text": "after probe"})).await.unwrap(), "after probe");
    d.shutdown().await;
}
