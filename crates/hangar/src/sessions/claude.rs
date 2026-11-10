//! Drives one headless Claude Code process over its stream-json protocol.
//!
//! ```text
//! claude --output-format stream-json --verbose --input-format stream-json
//!        --permission-prompt-tool stdio --permission-mode <mode> [--model M] [--resume=<id>]
//! ```
//!
//! Stdin carries user messages and control requests (initialize, interrupt,
//! set_permission_mode, mcp_status) plus our answers to `can_use_tool`.
//! Stdout carries assistant/user/result/system messages and control traffic.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

pub struct Launch {
    pub program: PathBuf,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub permission_mode: String,
    pub resume: Option<String>,
    /// Hangar's id for the session; exported as `HANGAR_SESSION`.
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub suggestions: Option<Value>,
    pub description: Option<String>,
}

pub enum DriverEvent {
    Message { session_id: String, msg: Value },
    Permission { session_id: String, request: PermissionRequest },
    McpStatus { session_id: String, servers: Vec<Value> },
    Exited { session_id: String, pid: u32, code: Option<i32>, stderr: Vec<String> },
}

pub struct Driver {
    pub pid: u32,
    tx: mpsc::UnboundedSender<String>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

impl Driver {
    pub fn args(l: &Launch) -> Vec<String> {
        let mut args: Vec<String> = ["--output-format", "stream-json", "--verbose", "--input-format", "stream-json", "--permission-prompt-tool", "stdio", "--permission-mode"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.push(l.permission_mode.clone());
        if let Some(m) = &l.model {
            args.push("--model".into());
            args.push(m.clone());
        }
        if let Some(r) = &l.resume {
            args.push(format!("--resume={r}"));
        }
        args
    }

    pub fn spawn(l: Launch, events: mpsc::UnboundedSender<DriverEvent>) -> Result<Driver> {
        let mut cmd = Command::new(&l.program);
        cmd.args(Self::args(&l))
            .current_dir(&l.cwd)
            .env(hangar_proto::ENV_SESSION, &l.session_id)
            // Started from inside a Claude Code session, the daemon inherits
            // this marker; a child claude would think it is nested.
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let mut child = cmd.spawn().with_context(|| format!("starting {}", l.program.display()))?;
        let pid = child.id().unwrap_or(0);
        let mut stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();

        // stdin writer
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        let id = l.session_id.clone();
        let reply = tx.clone();
        let ev = events.clone();
        // stdout reader
        tokio::spawn(async move {
            let mut reader = BufReader::with_capacity(256 * 1024, stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim();
                if !text.starts_with('{') {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(text) else { continue };
                route(&id, msg, &ev, &reply);
            }
        });

        let ring = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let ring_w = ring.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut r = ring_w.lock().unwrap();
                if r.len() >= 50 {
                    r.pop_front();
                }
                r.push_back(line);
            }
        });

        let id = l.session_id.clone();
        tokio::spawn(async move {
            let code = tokio::select! {
                s = child.wait() => s.ok().and_then(|s| s.code()),
                _ = kill_rx => {
                    let _ = child.start_kill();
                    child.wait().await.ok().and_then(|s| s.code())
                }
            };
            let stderr = ring.lock().unwrap().iter().cloned().collect();
            let _ = events.send(DriverEvent::Exited { session_id: id, pid, code, stderr });
        });

        let driver = Driver { pid, tx, kill: Mutex::new(Some(kill_tx)) };
        driver.control("init", json!({"subtype": "initialize", "hooks": null}));
        // Ask which MCP servers connected, once they have had time to start.
        let tx = driver.tx.clone();
        tokio::spawn(async move {
            for (i, wait) in [3u64, 7, 20].into_iter().enumerate() {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                let line = control_line(&format!("mcp-{i}"), json!({"subtype": "mcp_status"}));
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(driver)
    }

    fn write(&self, v: &Value) {
        let mut line = serde_json::to_string(v).unwrap_or_default();
        line.push('\n');
        let _ = self.tx.send(line);
    }

    fn control(&self, request_id: &str, request: Value) {
        let _ = self.tx.send(control_line(request_id, request));
    }

    pub fn send_user(&self, text: &str) {
        self.write(&json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null, "session_id": "default"}));
    }

    pub fn allow(&self, request_id: &str, input: &Value, updated_permissions: Option<Value>) {
        let mut response = json!({"behavior": "allow", "updatedInput": input});
        if let Some(p) = updated_permissions.filter(|p| p.as_array().is_some_and(|a| !a.is_empty())) {
            response["updatedPermissions"] = p;
        }
        self.write(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}}));
    }

    pub fn deny(&self, request_id: &str, message: &str) {
        self.write(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"behavior": "deny", "message": message}}}));
    }

    pub fn interrupt(&self) {
        self.control(&format!("int-{}", crate::pool::upstream::now_ms()), json!({"subtype": "interrupt"}));
    }

    pub fn set_permission_mode(&self, mode: &str) {
        self.control(&format!("mode-{}", crate::pool::upstream::now_ms()), json!({"subtype": "set_permission_mode", "mode": mode}));
    }

    pub fn kill(&self) {
        if let Some(k) = self.kill.lock().unwrap().take() {
            let _ = k.send(());
        }
    }
}

fn control_line(request_id: &str, request: Value) -> String {
    let mut s = serde_json::to_string(&json!({"type": "control_request", "request_id": request_id, "request": request})).unwrap_or_default();
    s.push('\n');
    s
}

fn route(id: &str, msg: Value, ev: &mpsc::UnboundedSender<DriverEvent>, reply: &mpsc::UnboundedSender<String>) {
    match msg["type"].as_str().unwrap_or_default() {
        "control_request" => {
            let request_id = msg["request_id"].as_str().unwrap_or_default().to_string();
            let req = &msg["request"];
            if req["subtype"] == "can_use_tool" {
                let request = PermissionRequest {
                    request_id,
                    tool_name: req["tool_name"].as_str().unwrap_or("tool").to_string(),
                    input: req["input"].clone(),
                    suggestions: req.get("permission_suggestions").filter(|v| !v.is_null()).cloned(),
                    description: req["description"].as_str().or(req["title"].as_str()).map(String::from),
                };
                let _ = ev.send(DriverEvent::Permission { session_id: id.to_string(), request });
            } else {
                // Hooks and SDK MCP servers are not registered, so nothing else should come.
                let err = json!({"type": "control_response", "response": {"subtype": "error", "request_id": request_id, "error": "not supported by hangar"}});
                let mut line = serde_json::to_string(&err).unwrap_or_default();
                line.push('\n');
                let _ = reply.send(line);
            }
        }
        "control_response" => {
            let r = &msg["response"];
            if r["request_id"].as_str().is_some_and(|s| s.starts_with("mcp-")) {
                let servers = r["response"]["mcpServers"].as_array().cloned().unwrap_or_default();
                let servers = servers.into_iter().map(|s| json!({"name": s["name"], "status": s["status"]})).collect();
                let _ = ev.send(DriverEvent::McpStatus { session_id: id.to_string(), servers });
            }
        }
        "control_cancel_request" | "keep_alive" => {}
        _ => {
            let _ = ev.send(DriverEvent::Message { session_id: id.to_string(), msg });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_cli_arguments() {
        let l = Launch {
            program: "claude".into(),
            cwd: ".".into(),
            model: Some("haiku".into()),
            permission_mode: "acceptEdits".into(),
            resume: Some("abc".into()),
            session_id: "s1".into(),
        };
        let a = Driver::args(&l);
        assert!(a.windows(2).any(|w| w == ["--permission-prompt-tool", "stdio"]));
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(a.contains(&"--resume=abc".to_string()));
    }

    #[test]
    fn routes_permission_requests_and_status() {
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
        let req = json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules"}],"tool_use_id":"t"}});
        route("s", req, &ev_tx, &reply_tx);
        match ev_rx.try_recv().unwrap() {
            DriverEvent::Permission { request, .. } => {
                assert_eq!(request.tool_name, "Bash");
                assert_eq!(request.input["command"], "ls");
                assert!(request.suggestions.is_some());
            }
            _ => panic!("expected a permission request"),
        }
        route("s", json!({"type":"control_request","request_id":"h","request":{"subtype":"hook_callback"}}), &ev_tx, &reply_tx);
        assert!(reply_rx.try_recv().unwrap().contains("\"subtype\":\"error\""), "unknown requests are refused, not left hanging");
        route("s", json!({"type":"control_response","response":{"request_id":"mcp-0","subtype":"success","response":{"mcpServers":[{"name":"a","status":"connected","config":{}}]}}}), &ev_tx, &reply_tx);
        assert!(matches!(ev_rx.try_recv().unwrap(), DriverEvent::McpStatus { servers, .. } if servers.len() == 1));
        route("s", json!({"type":"assistant","message":{}}), &ev_tx, &reply_tx);
        assert!(matches!(ev_rx.try_recv().unwrap(), DriverEvent::Message { .. }));
    }
}
