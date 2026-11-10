//! A stand-in for `claude` in stream-json mode, for tests.
//!
//! Each user message gets `system/init`, an assistant reply and a `result`.
//! Message text controls behavior:
//! * contains `use tool`: asks permission for `Bash {command: "ls"}` first;
//! * contains `slow`: waits 1.5 s before answering;
//! * contains `crash`: exits with code 5.
//! `FAKE_CLAUDE_LOG=<file>` appends the argv of every start to that file.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn out(v: Value) {
    let mut s = serde_json::to_string(&v).unwrap();
    s.push('\n');
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes());
    let _ = o.flush();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(log) = std::env::var("FAKE_CLAUDE_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{}", args.join(" "));
        }
    }
    let session_id = args
        .iter()
        .find_map(|a| a.strip_prefix("--resume="))
        .map(String::from)
        .unwrap_or_else(|| format!("fake-{}", std::process::id()));
    let mode = args.iter().position(|a| a == "--permission-mode").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "default".into());

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut turn = 0;
    while let Some(Ok(line)) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        match msg["type"].as_str().unwrap_or_default() {
            "control_request" => {
                let id = msg["request_id"].clone();
                let body = match msg["request"]["subtype"].as_str().unwrap_or_default() {
                    "mcp_status" => json!({"mcpServers": [{"name": "fake-server", "status": "connected"}]}),
                    _ => json!({}),
                };
                out(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": body}}));
            }
            "user" => {
                turn += 1;
                let text = msg["message"]["content"].as_str().unwrap_or_default().to_string();
                out(json!({"type": "system", "subtype": "init", "session_id": session_id, "model": "fake-model", "permissionMode": mode, "mcp_servers": [], "tools": []}));
                if text.contains("crash") {
                    std::process::exit(5);
                }
                if text.contains("slow") {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
                let mut reply = format!("echo: {text}");
                if text.contains("use tool") {
                    out(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "tu1", "name": "Bash", "input": {"command": "ls"}}]}, "session_id": session_id}));
                    out(json!({"type": "control_request", "request_id": format!("perm-{turn}"), "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}, "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "ls"}], "behavior": "allow", "destination": "session"}], "tool_use_id": "tu1"}}));
                    // Wait for the decision.
                    let decision = loop {
                        let Some(Ok(l)) = lines.next() else { return };
                        let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
                        if v["type"] == "control_response" {
                            break v;
                        }
                    };
                    let behavior = decision["response"]["response"]["behavior"].as_str().unwrap_or("?").to_string();
                    let always = decision["response"]["response"]["updatedPermissions"].is_array();
                    out(json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "tu1", "content": if behavior == "allow" { "file.txt" } else { "denied" }}]}, "session_id": session_id}));
                    reply = format!("{behavior}{}: ls", if always { " always" } else { "" });
                }
                out(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": reply}]}, "session_id": session_id}));
                out(json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 1, "total_cost_usd": 0.01, "duration_ms": 5, "session_id": session_id, "result": reply}));
            }
            _ => {}
        }
    }
}
