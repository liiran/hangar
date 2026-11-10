//! A scriptable MCP server for tests.
//!
//! Tools: `echo`, `pid`, `counter`, `slow` (progress), `crash`,
//! `notify_changed`, `ask_roots`, `big`, `touch` (resource update), `stats`.
//! Flags: `--init-delay-ms N`, `--fail-init`, `--exit-after-init`.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Stats {
    init_calls: u64,
    tools_list_calls: u64,
    subscribe_calls: u64,
    unsubscribe_calls: u64,
    cancelled: Vec<Value>,
    client_capabilities: Value,
}

struct Server {
    out: Mutex<std::io::Stdout>,
    stats: Mutex<Stats>,
    counter: AtomicU64,
    next_server_id: AtomicU64,
    /// Server→client requests waiting for an answer: our id -> original tools/call id.
    awaiting: Mutex<Vec<(String, Value)>>,
}

impl Server {
    fn send(&self, v: &Value) {
        let mut line = serde_json::to_string(v).unwrap();
        line.push('\n');
        let mut out = self.out.lock().unwrap();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }
    fn result(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
    fn error(&self, id: &Value, code: i64, message: &str) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }
    fn text(&self, id: &Value, text: String) {
        self.result(id, json!({"content": [{"type": "text", "text": text}]}));
    }
}

fn tools() -> Value {
    let t = |name: &str| json!({"name": name, "description": name, "inputSchema": {"type": "object"}});
    json!({"tools": [t("echo"), t("pid"), t("counter"), t("slow"), t("crash"), t("notify_changed"), t("ask_roots"), t("big"), t("touch"), t("stats")]})
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<u64>().ok());
    let init_delay = value("--init-delay-ms").unwrap_or(0);
    let fail_init = flag("--fail-init");
    let exit_after_init = flag("--exit-after-init");
    let stderr_garbage = flag("--stderr-garbage");

    let server = Arc::new(Server {
        out: Mutex::new(std::io::stdout()),
        stats: Mutex::new(Stats::default()),
        counter: AtomicU64::new(0),
        next_server_id: AtomicU64::new(1),
        awaiting: Mutex::new(Vec::new()),
    });
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let method = msg.get("method").and_then(Value::as_str).map(String::from);
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        match (method.as_deref(), id) {
            (Some("initialize"), Some(id)) => {
                std::thread::sleep(Duration::from_millis(init_delay));
                {
                    let mut st = server.stats.lock().unwrap();
                    st.init_calls += 1;
                    st.client_capabilities = msg.pointer("/params/capabilities").cloned().unwrap_or(Value::Null);
                }
                if fail_init {
                    server.error(&id, -32603, "refusing to initialize");
                    continue;
                }
                server.result(
                    &id,
                    json!({
                        "protocolVersion": msg.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                        "capabilities": {"tools": {"listChanged": true}, "resources": {"subscribe": true, "listChanged": true}, "logging": {}},
                        "serverInfo": {"name": "fake-mcp", "version": "1.0.0"},
                        "instructions": "a fake server"
                    }),
                );
                if exit_after_init {
                    std::process::exit(4);
                }
            }
            (Some("tools/list"), Some(id)) => {
                server.stats.lock().unwrap().tools_list_calls += 1;
                server.result(&id, tools());
            }
            (Some("resources/list"), Some(id)) => server.result(&id, json!({"resources": [{"uri": "mem://a", "name": "a"}]})),
            (Some("resources/subscribe"), Some(id)) => {
                server.stats.lock().unwrap().subscribe_calls += 1;
                if msg.pointer("/params/uri").and_then(Value::as_str) == Some("mem://deny") {
                    server.error(&id, -32602, "cannot subscribe to that");
                } else {
                    server.result(&id, json!({}));
                }
            }
            (Some("resources/unsubscribe"), Some(id)) => {
                server.stats.lock().unwrap().unsubscribe_calls += 1;
                server.result(&id, json!({}));
            }
            (Some("ping"), Some(id)) => server.result(&id, json!({})),
            (Some("tools/call"), Some(id)) => {
                if stderr_garbage {
                    // 256 KB of invalid UTF-8: more than a pipe holds, so this
                    // blocks forever unless someone keeps draining stderr.
                    let junk = vec![0xffu8; 256 * 1024];
                    let _ = std::io::stderr().write_all(&junk);
                    let _ = std::io::stderr().write_all(b"\n");
                }
                call_tool(&server, id, &msg)
            }
            (Some("notifications/cancelled"), None) => {
                server.stats.lock().unwrap().cancelled.push(msg.pointer("/params/requestId").cloned().unwrap_or(Value::Null));
            }
            (Some(_), Some(id)) => server.error(&id, -32601, "method not found"),
            (Some(_), None) => {}
            (None, Some(id)) => {
                // A response to one of our own requests (roots/list).
                let key = id.to_string();
                let original = {
                    let mut aw = server.awaiting.lock().unwrap();
                    aw.iter().position(|(k, _)| *k == key).map(|i| aw.remove(i).1)
                };
                if let Some(original) = original {
                    let body = msg.get("result").or_else(|| msg.get("error")).cloned().unwrap_or(Value::Null);
                    server.text(&original, body.to_string());
                }
            }
            (None, None) => {}
        }
    }
}

fn call_tool(server: &Arc<Server>, id: Value, msg: &Value) {
    let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or_default().to_string();
    let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
    match name.as_str() {
        "echo" => server.text(&id, args.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
        "pid" => server.text(&id, std::process::id().to_string()),
        "counter" => server.text(&id, (server.counter.fetch_add(1, Ordering::SeqCst) + 1).to_string()),
        "slow" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(200);
            let steps = args.get("steps").and_then(Value::as_u64).unwrap_or(3);
            let token = msg.pointer("/params/_meta/progressToken").cloned();
            let tag = args.get("tag").and_then(Value::as_str).unwrap_or("").to_string();
            let server = server.clone();
            std::thread::spawn(move || {
                for i in 1..=steps {
                    std::thread::sleep(Duration::from_millis(ms / steps.max(1)));
                    if let Some(t) = &token {
                        server.send(&json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": t, "progress": i, "total": steps, "message": tag}}));
                    }
                }
                server.text(&id, format!("done {tag}"));
            });
        }
        "crash" => std::process::exit(3),
        "notify_changed" => {
            server.send(&json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}));
            server.text(&id, "notified".into());
        }
        "ask_roots" => {
            let sid = format!("srv-{}", server.next_server_id.fetch_add(1, Ordering::SeqCst));
            let sid_value = json!(sid);
            server.awaiting.lock().unwrap().push((sid_value.to_string(), id));
            server.send(&json!({"jsonrpc": "2.0", "id": sid_value, "method": "roots/list"}));
        }
        "big" => {
            let kb = args.get("kb").and_then(Value::as_u64).unwrap_or(1024) as usize;
            server.text(&id, "x".repeat(kb * 1024));
        }
        "touch" => {
            let uri = args.get("uri").and_then(Value::as_str).unwrap_or("mem://a");
            server.send(&json!({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}}));
            server.text(&id, "touched".into());
        }
        "stats" => {
            let st = server.stats.lock().unwrap();
            let v = json!({
                "pid": std::process::id(),
                "init_calls": st.init_calls,
                "tools_list_calls": st.tools_list_calls,
                "subscribe_calls": st.subscribe_calls,
                "unsubscribe_calls": st.unsubscribe_calls,
                "cancelled": st.cancelled,
                "client_capabilities": st.client_capabilities,
            });
            drop(st);
            server.text(&id, v.to_string());
        }
        _ => server.error(&id, -32602, "unknown tool"),
    }
}
