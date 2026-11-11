//! A stand-in for `claude` in stream-json mode, for tests. It mirrors what
//! the real CLI does where hangar depends on it:
//!
//! * each user message gets `system/init`, an assistant reply and a `result`;
//! * `total_cost_usd` is a running total for the process (0.01 per turn);
//! * permission suggestions target `localSettings`, as the real CLI's often do.
//!
//! Message text controls behavior:
//! * `use tool`: asks permission for `Bash {command: "ls"}` first; an
//!   interrupt while waiting withdraws the request (`control_cancel_request`);
//! * `slow`: waits 1.5 s before answering;
//! * `which model`: answers `model: <name>` (from `--model` or `set_model`);
//! * `crash`: exits with code 5.
//!
//! `FAKE_CLAUDE_LOG=<file>` appends the argv of every start to that file.
//! `--resume=bad` fails at startup like a conversation that cannot be resumed.

use serde_json::{Value, json};
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
    if let Ok(log) = std::env::var("FAKE_CLAUDE_LOG")
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log)
    {
        let _ = writeln!(f, "{}", args.join(" "));
    }
    let resume = args.iter().find_map(|a| a.strip_prefix("--resume=")).map(String::from);
    if resume.as_deref() == Some("bad") {
        eprintln!("No conversation found with session ID: bad");
        std::process::exit(1);
    }
    let session_id = resume.unwrap_or_else(|| format!("fake-{}", std::process::id()));
    let mode = args.iter().position(|a| a == "--permission-mode").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "default".into());
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mut model = flag("--model").unwrap_or_else(|| "fake-model".into());

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut turn: u64 = 0;
    let result = |turn: u64, reply: &str| {
        out(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": reply}]}, "session_id": session_id}));
        out(
            json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 1, "total_cost_usd": 0.01 * turn as f64, "duration_ms": 5, "session_id": session_id, "result": reply}),
        );
    };
    while let Some(Ok(line)) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        match msg["type"].as_str().unwrap_or_default() {
            "control_request" => {
                let body = match msg["request"]["subtype"].as_str().unwrap_or_default() {
                    "mcp_status" => json!({"mcpServers": [{"name": "fake-server", "status": "connected"}]}),
                    "set_model" => {
                        model = msg["request"]["model"].as_str().unwrap_or("fake-model").to_string();
                        json!({"model": model})
                    }
                    _ => json!({}),
                };
                out(json!({"type": "control_response", "response": {"subtype": "success", "request_id": msg["request_id"], "response": body}}));
            }
            "user" => {
                turn += 1;
                let text = msg["message"]["content"].as_str().unwrap_or_default().to_string();
                out(
                    json!({"type": "system", "subtype": "init", "session_id": session_id, "model": model, "permissionMode": mode, "mcp_servers": [], "tools": []}),
                );
                if text.contains("crash") {
                    std::process::exit(5);
                }
                if text.contains("which model") {
                    result(turn, &format!("model: {model}"));
                    continue;
                }
                if text.contains("slow") {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
                if !text.contains("use tool") {
                    result(turn, &format!("echo: {text}"));
                    continue;
                }
                let perm = format!("perm-{turn}");
                out(
                    json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "tu1", "name": "Bash", "input": {"command": "ls"}}]}, "session_id": session_id}),
                );
                out(
                    json!({"type": "control_request", "request_id": perm, "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}, "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "ls"}], "behavior": "allow", "destination": "localSettings"}], "tool_use_id": "tu1"}}),
                );
                // Wait for the decision, or an interrupt.
                let reply = loop {
                    let Some(Ok(l)) = lines.next() else { return };
                    let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
                    if v["type"] == "control_request" && v["request"]["subtype"] == "interrupt" {
                        out(json!({"type": "control_response", "response": {"subtype": "success", "request_id": v["request_id"], "response": {}}}));
                        out(json!({"type": "control_cancel_request", "request_id": perm}));
                        break "interrupted".to_string();
                    }
                    if v["type"] == "control_response" {
                        let r = &v["response"]["response"];
                        let behavior = r["behavior"].as_str().unwrap_or("?").to_string();
                        let remembered = r["updatedPermissions"][0]["destination"].as_str().map(|d| format!(" always({d})")).unwrap_or_default();
                        out(
                            json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "tu1", "content": if behavior == "allow" { "file.txt" } else { "denied" }}]}, "session_id": session_id}),
                        );
                        break format!("{behavior}{remembered}: ls");
                    }
                };
                result(turn, &reply);
            }
            _ => {}
        }
    }
}
