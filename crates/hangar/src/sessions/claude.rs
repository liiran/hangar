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
//! Every event is tagged with the process id, and `Exited` is sent only after
//! the last line of output, so the manager never mixes up two processes.

use crate::procs;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Mutex;
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

/// What a driver reports. Every variant names the process it came from.
pub enum DriverEvent {
    Message {
        session_id: String,
        pid: u32,
        msg: Value,
    },
    Permission {
        session_id: String,
        pid: u32,
        request: PermissionRequest,
    },
    /// Claude withdrew a permission request (for example after an interrupt).
    PermissionCancelled {
        session_id: String,
        pid: u32,
        request_id: String,
    },
    McpStatus {
        session_id: String,
        pid: u32,
        servers: Vec<Value>,
    },
    Exited {
        session_id: String,
        pid: u32,
        code: Option<i32>,
        stderr: Vec<String>,
    },
}

pub struct Driver {
    pub pid: u32,
    tx: mpsc::UnboundedSender<String>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

impl Driver {
    pub fn args(l: &Launch) -> Vec<String> {
        let mut args: Vec<String> =
            ["--output-format", "stream-json", "--verbose", "--input-format", "stream-json", "--permission-prompt-tool", "stdio", "--permission-mode"]
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
        procs::hide_window(&mut cmd);
        let mut child = cmd.spawn().with_context(|| format!("starting {}", l.program.display()))?;
        let pid = child.id().unwrap_or(0);
        // Everything the session starts (tools, servers) lives and dies with it.
        let job = procs::contain(&child);
        let mut stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        let id = l.session_id.clone();
        let reply = tx.clone();
        let ev = events.clone();
        let reader = tokio::spawn(async move {
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
                if let Ok(msg) = serde_json::from_str::<Value>(text) {
                    route(&id, pid, msg, &ev, &reply);
                }
            }
        });

        let ring: crate::pool::process::Ring = Default::default();
        crate::pool::process::drain(stderr, ring.clone(), "");

        let id = l.session_id.clone();
        tokio::spawn(async move {
            let code = tokio::select! {
                s = child.wait() => s.ok().and_then(|s| s.code()),
                _ = kill_rx => {
                    if let Some(job) = &job {
                        job.terminate();
                    }
                    let _ = child.start_kill();
                    child.wait().await.ok().and_then(|s| s.code())
                }
            };
            // Report the exit only after the last line of output.
            let _ = tokio::time::timeout(Duration::from_secs(5), reader).await;
            drop(job); // anything the session left running dies now
            let stderr = crate::pool::process::tail(&ring, 50);
            let _ = events.send(DriverEvent::Exited { session_id: id, pid, code, stderr });
        });

        let driver = Driver { pid, tx, kill: Mutex::new(Some(kill_tx)) };
        driver.control("init", json!({"subtype": "initialize", "hooks": null}));
        // Ask which MCP servers connected, once they have had time to start.
        let tx = driver.tx.clone();
        tokio::spawn(async move {
            for (i, wait) in [3u64, 7, 20].into_iter().enumerate() {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                if tx.send(control_line(&format!("mcp-{i}"), json!({"subtype": "mcp_status"}))).is_err() {
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

    /// Allows a tool call. With `remember`, Claude's suggested rules are
    /// applied to this session only; returns whether any were.
    pub fn allow(&self, request_id: &str, input: &Value, remember: Option<&Value>) -> bool {
        let mut response = json!({"behavior": "allow", "updatedInput": input});
        let rules = remember.and_then(session_only);
        let remembered = rules.is_some();
        if let Some(rules) = rules {
            response["updatedPermissions"] = rules;
        }
        self.write(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}}));
        remembered
    }

    pub fn deny(&self, request_id: &str, message: &str) {
        self.write(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"behavior": "deny", "message": message}}}));
    }

    pub fn interrupt(&self) {
        self.control(&format!("int-{}", crate::util::now_ms()), json!({"subtype": "interrupt"}));
    }

    pub fn set_permission_mode(&self, mode: &str) {
        self.control(&format!("mode-{}", crate::util::now_ms()), json!({"subtype": "set_permission_mode", "mode": mode}));
    }

    /// Switches the model from the next turn on; `None` is Claude Code's default.
    pub fn set_model(&self, model: Option<&str>) {
        let mut request = json!({"subtype": "set_model"});
        if let Some(m) = model {
            request["model"] = json!(m);
        }
        self.control(&format!("model-{}", crate::util::now_ms()), request);
    }

    /// Kills the process and everything it started.
    pub fn kill(&self) {
        if let Some(k) = self.kill.lock().unwrap().take() {
            let _ = k.send(());
        }
    }
}

/// Keeps only "allow" rule additions from Claude's suggestions and scopes
/// them to the session, whatever destination Claude suggested (it often
/// suggests saving them to settings files). Mode changes and extra
/// directories are dropped.
pub fn session_only(suggestions: &Value) -> Option<Value> {
    let rules: Vec<Value> = suggestions
        .as_array()?
        .iter()
        .filter(|s| s["type"] == "addRules" && s["behavior"] == "allow")
        .map(|s| {
            let mut s = s.clone();
            s["destination"] = json!("session");
            s
        })
        .collect();
    (!rules.is_empty()).then_some(Value::Array(rules))
}

fn control_line(request_id: &str, request: Value) -> String {
    let mut s = serde_json::to_string(&json!({"type": "control_request", "request_id": request_id, "request": request})).unwrap_or_default();
    s.push('\n');
    s
}

fn route(id: &str, pid: u32, msg: Value, ev: &mpsc::UnboundedSender<DriverEvent>, reply: &mpsc::UnboundedSender<String>) {
    let session_id = id.to_string();
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
                let _ = ev.send(DriverEvent::Permission { session_id, pid, request });
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
                let _ = ev.send(DriverEvent::McpStatus { session_id, pid, servers });
            }
        }
        "control_cancel_request" => {
            let request_id = msg["request_id"].as_str().unwrap_or_default().to_string();
            let _ = ev.send(DriverEvent::PermissionCancelled { session_id, pid, request_id });
        }
        "keep_alive" => {}
        _ => {
            let _ = ev.send(DriverEvent::Message { session_id, pid, msg });
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
    fn always_allow_never_writes_settings_files() {
        let suggestions = json!([
            {"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "ls"}], "behavior": "allow", "destination": "localSettings"},
            {"type": "setMode", "mode": "bypassPermissions", "destination": "userSettings"},
            {"type": "addDirectories", "directories": ["C:/"], "destination": "userSettings"}
        ]);
        let rules = session_only(&suggestions).unwrap();
        let rules = rules.as_array().unwrap();
        assert_eq!(rules.len(), 1, "mode changes and new directories are dropped");
        assert_eq!(rules[0]["destination"], "session");
        assert_eq!(session_only(&json!([{"type": "setMode", "mode": "plan"}])), None);
        assert_eq!(session_only(&Value::Null), None);
    }

    #[test]
    fn routes_permission_traffic_with_the_pid() {
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
        let req = json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules"}],"tool_use_id":"t"}});
        route("s", 42, req, &ev_tx, &reply_tx);
        match ev_rx.try_recv().unwrap() {
            DriverEvent::Permission { request, pid, .. } => {
                assert_eq!((request.tool_name.as_str(), pid), ("Bash", 42));
                assert_eq!(request.input["command"], "ls");
                assert!(request.suggestions.is_some());
            }
            _ => panic!("expected a permission request"),
        }
        route("s", 42, json!({"type":"control_request","request_id":"h","request":{"subtype":"hook_callback"}}), &ev_tx, &reply_tx);
        assert!(reply_rx.try_recv().unwrap().contains("\"subtype\":\"error\""), "unknown requests are refused, not left hanging");
        route("s", 42, json!({"type":"control_cancel_request","request_id":"r1"}), &ev_tx, &reply_tx);
        assert!(matches!(ev_rx.try_recv().unwrap(), DriverEvent::PermissionCancelled { request_id, .. } if request_id == "r1"));
        route(
            "s",
            42,
            json!({"type":"control_response","response":{"request_id":"mcp-0","subtype":"success","response":{"mcpServers":[{"name":"a","status":"connected","config":{}}]}}}),
            &ev_tx,
            &reply_tx,
        );
        assert!(matches!(ev_rx.try_recv().unwrap(), DriverEvent::McpStatus { servers, .. } if servers.len() == 1));
        route("s", 42, json!({"type":"assistant","message":{}}), &ev_tx, &reply_tx);
        assert!(matches!(ev_rx.try_recv().unwrap(), DriverEvent::Message { pid: 42, .. }));
    }
}
