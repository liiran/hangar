//! A minimal MCP client over a child process's stdio.
//!
//! The benchmark uses it to stand in for agent sessions, and the integration
//! tests use it to talk to servers through the shim. It answers the requests a
//! server may send to its client (`ping`, `roots/list`) the way an agent would.

use crate::jsonrpc::{self, Kind};
use crate::util::Locked;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Requests waiting for an answer. `None` once the server's stdout closed,
/// so later requests fail at once instead of waiting for their timeout.
type Pending = Arc<Mutex<Option<HashMap<String, oneshot::Sender<Value>>>>>;

type SharedStdin = Arc<tokio::sync::Mutex<Option<ChildStdin>>>;

pub struct McpClient {
    stdin: SharedStdin,
    pending: Pending,
    next_id: AtomicU64,
    notifications: tokio::sync::Mutex<mpsc::UnboundedReceiver<Value>>,
    server_requests: Arc<Mutex<Vec<Value>>>,
    child: Mutex<Option<Child>>,
    pid: u32,
}

impl McpClient {
    /// Starts `cmd` with piped stdio. `roots` answers `roots/list`.
    pub fn spawn(mut cmd: Command, roots: Vec<String>) -> Result<McpClient> {
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn().context("spawning MCP server")?;
        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let server_requests = Arc::new(Mutex::new(Vec::new()));
        let (note_tx, note_rx) = mpsc::unbounded_channel();
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Value>();

        let stdin: SharedStdin = Arc::new(tokio::sync::Mutex::new(Some(stdin)));
        let reply_stdin = stdin.clone();
        let client = McpClient {
            stdin,
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            notifications: tokio::sync::Mutex::new(note_rx),
            server_requests: server_requests.clone(),
            child: Mutex::new(Some(child)),
            pid,
        };

        let roots_value: Vec<Value> = roots.iter().map(|r| json!({"uri": file_uri(r), "name": r})).collect();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // Servers sometimes print junk; skip it rather than give up.
                let line = String::from_utf8_lossy(&buf);
                let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
                match jsonrpc::classify(&msg) {
                    Kind::Response { id } => {
                        let key = jsonrpc::id_key(id);
                        let waiter = pending.locked().as_mut().and_then(|p| p.remove(&key));
                        if let Some(tx) = waiter {
                            let _ = tx.send(msg);
                        }
                    }
                    Kind::Request { id, method } => {
                        server_requests.locked().push(msg.clone());
                        let reply = match method {
                            "ping" => jsonrpc::result(id.clone(), json!({})),
                            "roots/list" => jsonrpc::result(id.clone(), json!({"roots": roots_value})),
                            _ => jsonrpc::error(id.clone(), jsonrpc::METHOD_NOT_FOUND, "not supported by this client"),
                        };
                        let _ = reply_tx.send(reply);
                    }
                    Kind::Notification { .. } => {
                        let _ = note_tx.send(msg);
                    }
                    Kind::Invalid => {}
                }
            }
            // Fail every request still waiting, and every later one.
            pending.locked().take();
        });

        // Replies to server requests go through the same stdin.
        tokio::spawn(async move {
            while let Some(reply) = reply_rx.recv().await {
                let mut guard = reply_stdin.lock().await;
                let Some(stdin) = guard.as_mut() else { break };
                if stdin.write_all(jsonrpc::to_line(&reply).as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        Ok(client)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn send(&self, msg: &Value) -> Result<()> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or_else(|| anyhow!("stdin closed"))?;
        stdin.write_all(jsonrpc::to_line(msg).as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Sends a request with an explicit id and waits for the full response message.
    pub async fn request_with_id(&self, id: Value, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let key = jsonrpc::id_key(&id);
        let (tx, rx) = oneshot::channel();
        match self.pending.locked().as_mut() {
            Some(p) => p.insert(key.clone(), tx),
            None => bail!("server closed before {method}"),
        };
        let forget = || {
            if let Some(p) = self.pending.locked().as_mut() {
                p.remove(&key);
            }
        };
        if let Err(e) = self.send(&jsonrpc::request(id, method, params)).await {
            forget();
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(msg)) => Ok(msg),
            Ok(Err(_)) => bail!("server closed before answering {method}"),
            Err(_) => {
                forget();
                bail!("timed out waiting for {method}")
            }
        }
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = json!(self.next_id.fetch_add(1, Ordering::SeqCst));
        self.request_with_id(id, method, params, timeout).await
    }

    /// Like [`request`](Self::request) but returns `result` and turns errors into `Err`.
    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let msg = self.request(method, params, timeout).await?;
        if let Some(err) = msg.get("error") {
            bail!("{method} failed: {err}");
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        self.send(&jsonrpc::notification(method, params)).await
    }

    /// `initialize` + `notifications/initialized`. Returns the initialize result.
    pub async fn initialize(&self, client_name: &str, timeout: Duration) -> Result<Value> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"roots": {"listChanged": true}},
            "clientInfo": {"name": client_name, "version": env!("CARGO_PKG_VERSION")}
        });
        let result = self.call("initialize", params, timeout).await?;
        self.notify("notifications/initialized", None).await?;
        Ok(result)
    }

    pub async fn next_notification(&self, timeout: Duration) -> Option<Value> {
        let mut rx = self.notifications.lock().await;
        tokio::time::timeout(timeout, rx.recv()).await.ok().flatten()
    }

    /// Waits for a notification with the given method, skipping others.
    pub async fn wait_notification(&self, method: &str, timeout: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let msg = self.next_notification(left).await?;
            if msg.get("method").and_then(Value::as_str) == Some(method) {
                return Some(msg);
            }
        }
    }

    pub fn server_requests(&self) -> Vec<Value> {
        self.server_requests.locked().clone()
    }

    /// Closes stdin, the normal way a client ends a stdio session.
    pub async fn close_stdin(&self) {
        self.stdin.lock().await.take();
    }

    /// Ends the session the way Node-based agents do: close stdin and kill the
    /// direct child process (its descendants are left to fend for themselves).
    pub async fn shutdown_like_agent(&self) {
        self.close_stdin().await;
        let child = self.child.locked().take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
}

pub fn file_uri(path: &str) -> String {
    let p = path.replace('\\', "/");
    if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
}
