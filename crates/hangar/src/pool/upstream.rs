//! One pooled MCP server process and everything needed to share it.
//!
//! An [`Upstream`] is an actor: a single task owns the child process and all
//! routing state, and the rest of the daemon talks to it through [`Cmd`]s.
//!
//! Routing rules (shared servers):
//! * Each session's request id is replaced with one that is unique on the
//!   upstream and swapped back on the response.
//! * `_meta.progressToken` and `notifications/cancelled` are remapped the
//!   same way, so sessions never see each other's progress or cancel each
//!   other's requests.
//! * `resources/subscribe` is reference counted; updates go to subscribers.
//! * `tools/list`, `prompts/list`, `resources/list` and
//!   `resources/templates/list` are cached until the server says they changed.
//! * The upstream is initialized once, without per-client capabilities
//!   (roots, sampling, elicitation), so it never asks one session a question
//!   meant for another.
//!
//! A `session`-scoped upstream has exactly one session and passes everything
//! through, including requests from the server to the client.

use super::{PoolKey, PoolOptions};
use crate::jsonrpc::{self, Kind};
use crate::policy::Share;
use crate::procs::Job;
use hangar_proto::ServerSpec;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

const MAX_QUEUED: usize = 10_000;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const MAX_CRASHES: usize = 5;
const FAILED_RETRY: Duration = Duration::from_secs(30);
const STDERR_LINES: usize = 200;

/// A session attached to an upstream.
pub struct ConnInfo {
    pub conn: u64,
    pub name: String,
    pub session: Option<String>,
    pub client_pid: Option<u32>,
    pub shim_pid: u32,
    /// Lines to write to the session's socket.
    pub tx: mpsc::UnboundedSender<String>,
    pub attached_at_ms: u64,
}

#[derive(Debug)]
pub struct Rejected;

pub enum Cmd {
    /// `init_id` is the id of the session's `initialize` request, or `None`
    /// when a shim reconnects and the session is already initialized.
    /// `init_params` are the session's own initialize params (used by
    /// multi-session HTTP servers, where every session initializes itself).
    Attach { info: ConnInfo, init_id: Option<Value>, init_params: Value, reply: oneshot::Sender<Result<(), Rejected>> },
    Client { conn: u64, msg: Value },
    Detach { conn: u64 },
    Stop { reply: Option<oneshot::Sender<()>> },
    Stats { reply: oneshot::Sender<UpstreamStats> },
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRef {
    pub conn: u64,
    pub name: String,
    pub session: Option<String>,
    pub client_pid: Option<u32>,
    pub shim_pid: u32,
    pub attached_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpstreamStats {
    pub id: u64,
    pub name: String,
    pub share: Share,
    pub scope: String,
    pub command: String,
    pub pid: Option<u32>,
    pub state: String,
    pub sessions: Vec<SessionRef>,
    pub started_at_ms: Option<u64>,
    pub restarts: u64,
    pub requests: u64,
    pub cache_hits: u64,
    pub last_error: Option<String>,
    pub stderr_tail: Vec<String>,
    /// The command line actually running (differs from `command` on the npx fast path).
    pub launch: Option<String>,
    pub fast_path: bool,
}

/// Cheap handle to an upstream actor.
#[derive(Clone)]
pub struct Upstream {
    pub id: u64,
    pub key: PoolKey,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Upstream {
    pub(crate) fn from_parts(id: u64, key: PoolKey, tx: mpsc::UnboundedSender<Cmd>) -> Upstream {
        Upstream { id, key, tx }
    }

    pub fn send(&self, cmd: Cmd) -> bool {
        self.tx.send(cmd).is_ok()
    }

    pub fn is_alive(&self) -> bool {
        !self.tx.is_closed()
    }

    pub async fn stats(&self) -> Option<UpstreamStats> {
        let (reply, rx) = oneshot::channel();
        if !self.send(Cmd::Stats { reply }) {
            return None;
        }
        rx.await.ok()
    }
}

pub struct Spawn {
    pub id: u64,
    pub key: PoolKey,
    pub name: String,
    pub share: Share,
    pub spec: ServerSpec,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    /// Params of the first session's `initialize` request.
    pub client_init: Value,
    pub opts: PoolOptions,
    pub on_exit: Weak<dyn Fn(u64, &PoolKey) + Send + Sync>,
}

pub fn start(spawn: Spawn) -> Upstream {
    let (tx, rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let handle = Upstream { id: spawn.id, key: spawn.key.clone(), tx };
    let init_params = upstream_init_params(&spawn.client_init, spawn.share);
    let actor = Actor {
        id: spawn.id,
        key: spawn.key,
        name: spawn.name,
        share: spawn.share,
        spec: spawn.spec,
        cwd: spawn.cwd,
        env: spawn.env,
        init_params,
        opts: spawn.opts,
        on_exit: spawn.on_exit,
        rx,
        ev_tx,
        ev_rx,
        phase: Phase::Idle,
        proc_: None,
        generation: 0,
        init_result: None,
        init_upstream_id: None,
        init_deadline: None,
        was_ready: false,
        next_id: 1,
        conns: HashMap::new(),
        waiting_init: Vec::new(),
        queued: VecDeque::new(),
        pending: HashMap::new(),
        by_client: HashMap::new(),
        progress: HashMap::new(),
        subs: HashMap::new(),
        cache: HashMap::new(),
        crashes: VecDeque::new(),
        idle_since: None,
        started_at_ms: None,
        restarts: 0,
        requests: 0,
        cache_hits: 0,
        last_error: None,
        stderr: Arc::new(Mutex::new(VecDeque::new())),
        done: false,
        launch: None,
        fast_failed: false,
    };
    tokio::spawn(actor.run());
    handle
}

/// What the upstream sees as its client's `initialize` params.
fn upstream_init_params(client: &Value, share: Share) -> Value {
    let mut params = client.clone();
    if share.is_shared() {
        if let Some(obj) = params.as_object_mut() {
            // Per-client features cannot be shared, so the server never sees them.
            obj.insert("capabilities".into(), json!({}));
        }
    }
    params
}

enum Phase {
    Idle,
    Starting,
    Ready,
    Backoff(Instant),
    Failed { retry_at: Instant },
    Stopping,
}

impl Phase {
    fn label(&self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Starting => "starting",
            Phase::Ready => "ready",
            Phase::Backoff(_) => "restarting",
            Phase::Failed { .. } => "failed",
            Phase::Stopping => "stopping",
        }
    }
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    job: Option<Job>,
    pid: u32,
}

enum ProcEvent {
    Line(u64, String),
    Eof(u64),
}

struct Pending {
    conn: u64,
    client_id: Value,
    progress_token: Option<String>,
    cache_key: Option<String>,
}

struct Actor {
    id: u64,
    key: PoolKey,
    name: String,
    share: Share,
    spec: ServerSpec,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    init_params: Value,
    opts: PoolOptions,
    on_exit: Weak<dyn Fn(u64, &PoolKey) + Send + Sync>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    ev_tx: mpsc::UnboundedSender<ProcEvent>,
    ev_rx: mpsc::UnboundedReceiver<ProcEvent>,
    phase: Phase,
    proc_: Option<Proc>,
    generation: u64,
    init_result: Option<Value>,
    init_upstream_id: Option<u64>,
    init_deadline: Option<Instant>,
    was_ready: bool,
    next_id: u64,
    conns: HashMap<u64, ConnInfo>,
    /// Sessions waiting for their `initialize` answer: (conn, client id).
    waiting_init: Vec<(u64, Value)>,
    /// Session messages that arrived while the server was not ready.
    queued: VecDeque<(u64, Value)>,
    /// Requests in flight, by upstream id. `conn == 0` marks our own requests.
    pending: HashMap<u64, Pending>,
    by_client: HashMap<(u64, String), u64>,
    progress: HashMap<String, (u64, Value)>,
    subs: HashMap<String, HashSet<u64>>,
    cache: HashMap<String, Value>,
    crashes: VecDeque<Instant>,
    idle_since: Option<Instant>,
    started_at_ms: Option<u64>,
    restarts: u64,
    requests: u64,
    cache_hits: u64,
    last_error: Option<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    done: bool,
    launch: Option<crate::launch::Launch>,
    /// The npx fast path failed once; use the configured command from now on.
    fast_failed: bool,
}

enum Wake {
    Cmd(Option<Cmd>),
    Proc(Option<ProcEvent>),
    Timer,
}

impl Actor {
    async fn run(mut self) {
        tracing::debug!(upstream = self.id, name = %self.name, scope = %self.key.scope, "upstream created");
        self.spawn_process().await;
        while !self.done {
            let deadline = self.next_deadline();
            let wake = tokio::select! {
                cmd = self.rx.recv() => Wake::Cmd(cmd),
                ev = self.ev_rx.recv() => Wake::Proc(ev),
                _ = sleep_until(deadline) => Wake::Timer,
            };
            match wake {
                Wake::Cmd(None) => {
                    self.stop_process().await;
                    self.done = true;
                }
                Wake::Cmd(Some(cmd)) => self.on_cmd(cmd).await,
                Wake::Proc(Some(ProcEvent::Line(g, line))) if g == self.generation => self.on_upstream_line(&line).await,
                Wake::Proc(Some(ProcEvent::Eof(g))) if g == self.generation => self.on_exit().await,
                Wake::Proc(_) => {}
                Wake::Timer => self.on_timer().await,
            }
        }
        if let Some(cb) = self.on_exit.upgrade() {
            cb(self.id, &self.key);
        }
        // Anyone who raced us to attach gets a clean rejection and retries.
        self.rx.close();
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Cmd::Attach { reply, .. } => {
                    let _ = reply.send(Err(Rejected));
                }
                Cmd::Stop { reply: Some(r) } => {
                    let _ = r.send(());
                }
                Cmd::Stats { reply } => {
                    let _ = reply.send(self.snapshot());
                }
                _ => {}
            }
        }
        tracing::debug!(upstream = self.id, name = %self.name, "upstream finished");
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut d: Option<Instant> = None;
        let mut take = |t: Instant| d = Some(d.map_or(t, |x| x.min(t)));
        match self.phase {
            Phase::Starting => {
                if let Some(t) = self.init_deadline {
                    take(t)
                }
            }
            Phase::Backoff(t) => take(t),
            _ => {}
        }
        if let Some(since) = self.idle_since {
            take(since + self.idle_limit());
        }
        d
    }

    fn idle_limit(&self) -> Duration {
        if self.share.is_shared() { self.opts.idle_timeout } else { self.opts.session_grace }
    }

    async fn on_timer(&mut self) {
        let now = Instant::now();
        if let Phase::Starting = self.phase {
            if self.init_deadline.is_some_and(|t| now >= t) {
                self.fail_start(format!("no answer to initialize after {}s", self.opts.init_timeout.as_secs())).await;
                return;
            }
        }
        if let Phase::Backoff(t) = self.phase {
            if now >= t {
                self.spawn_process().await;
                return;
            }
        }
        if self.conns.is_empty() && self.idle_since.is_some_and(|s| now >= s + self.idle_limit()) {
            tracing::info!(upstream = self.id, name = %self.name, "stopping idle server");
            self.stop_process().await;
            self.done = true;
        }
    }

    // ------------------------------------------------------------ commands

    async fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Attach { info, init_id, reply, .. } => {
                if matches!(self.phase, Phase::Stopping) || self.done {
                    let _ = reply.send(Err(Rejected));
                    return;
                }
                let conn = info.conn;
                self.conns.insert(conn, info);
                self.idle_since = None;
                if let Some(id) = init_id {
                    match (&self.phase, &self.init_result) {
                        (Phase::Ready, Some(result)) => {
                            let msg = jsonrpc::result(id, result.clone());
                            self.send_to(conn, &msg);
                        }
                        _ => self.waiting_init.push((conn, id)),
                    }
                }
                if let Phase::Failed { retry_at } = self.phase {
                    if Instant::now() >= retry_at {
                        self.spawn_process().await;
                    }
                }
                let _ = reply.send(Ok(()));
            }
            Cmd::Client { conn, msg } => {
                if self.conns.contains_key(&conn) {
                    self.on_client(conn, msg).await;
                }
            }
            Cmd::Detach { conn } => self.detach(conn).await,
            Cmd::Stop { reply } => {
                self.stop_process().await;
                self.done = true;
                if let Some(r) = reply {
                    let _ = r.send(());
                }
            }
            Cmd::Stats { reply } => {
                let _ = reply.send(self.snapshot());
            }
        }
    }

    fn snapshot(&self) -> UpstreamStats {
        let mut sessions: Vec<SessionRef> = self
            .conns
            .values()
            .map(|c| SessionRef {
                conn: c.conn,
                name: c.name.clone(),
                session: c.session.clone(),
                client_pid: c.client_pid,
                shim_pid: c.shim_pid,
                attached_at_ms: c.attached_at_ms,
            })
            .collect();
        sessions.sort_by_key(|s| s.conn);
        UpstreamStats {
            id: self.id,
            name: self.name.clone(),
            share: self.share,
            scope: self.key.scope.clone(),
            command: self.spec.display(),
            pid: self.proc_.as_ref().map(|p| p.pid),
            state: self.phase.label().to_string(),
            sessions,
            started_at_ms: self.started_at_ms,
            restarts: self.restarts,
            requests: self.requests,
            cache_hits: self.cache_hits,
            last_error: self.last_error.clone(),
            stderr_tail: self.stderr.lock().unwrap().iter().rev().take(20).rev().cloned().collect(),
            launch: self.launch.as_ref().map(|l| l.display()),
            fast_path: self.launch.as_ref().is_some_and(|l| l.fast),
        }
    }

    async fn detach(&mut self, conn: u64) {
        if self.conns.remove(&conn).is_none() {
            return;
        }
        self.waiting_init.retain(|(c, _)| *c != conn);
        self.queued.retain(|(c, _)| *c != conn);
        // Tell the server to stop work nobody is waiting for.
        let abandoned: Vec<u64> = self.pending.iter().filter(|(_, p)| p.conn == conn).map(|(id, _)| *id).collect();
        for uid in abandoned {
            if let Some(p) = self.pending.remove(&uid) {
                if let Some(t) = p.progress_token {
                    self.progress.remove(&t);
                }
                let cancel = jsonrpc::notification("notifications/cancelled", Some(json!({"requestId": uid, "reason": "client disconnected"})));
                self.write_upstream(&cancel).await;
            }
        }
        self.by_client.retain(|(c, _), _| *c != conn);
        self.progress.retain(|_, (c, _)| *c != conn);
        // Drop the session's subscriptions; unsubscribe upstream when nobody is left.
        let mut emptied = Vec::new();
        for (uri, set) in self.subs.iter_mut() {
            if set.remove(&conn) && set.is_empty() {
                emptied.push(uri.clone());
            }
        }
        for uri in emptied {
            self.subs.remove(&uri);
            if matches!(self.phase, Phase::Ready) {
                let uid = self.alloc_id();
                self.pending.insert(uid, Pending { conn: 0, client_id: Value::Null, progress_token: None, cache_key: None });
                self.write_upstream(&jsonrpc::request(json!(uid), "resources/unsubscribe", json!({"uri": uri}))).await;
            }
        }
        if self.conns.is_empty() {
            self.idle_since = Some(Instant::now());
        }
    }

    // ------------------------------------------------------- client → server

    async fn on_client(&mut self, conn: u64, msg: Value) {
        match self.phase {
            Phase::Ready => {}
            Phase::Failed { retry_at } if Instant::now() < retry_at => {
                if let Kind::Request { id, .. } = jsonrpc::classify(&msg) {
                    let reason = self.last_error.clone().unwrap_or_else(|| "server failed".into());
                    let err = jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, format!("{} is unavailable: {reason}", self.name));
                    self.send_to(conn, &err);
                }
                return;
            }
            Phase::Failed { .. } => {
                self.spawn_process().await;
                self.enqueue(conn, msg);
                return;
            }
            _ => {
                self.enqueue(conn, msg);
                return;
            }
        }
        self.route_client(conn, msg).await;
    }

    fn enqueue(&mut self, conn: u64, msg: Value) {
        if self.queued.len() >= MAX_QUEUED {
            if let Kind::Request { id, .. } = jsonrpc::classify(&msg) {
                let err = jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, "too many requests while the server is starting");
                self.send_to(conn, &err);
            }
            return;
        }
        self.queued.push_back((conn, msg));
    }

    async fn route_client(&mut self, conn: u64, mut msg: Value) {
        let shared = self.share.is_shared();
        let kind = match jsonrpc::classify(&msg) {
            Kind::Request { id, method } => Some((Some(id.clone()), method.to_string())),
            Kind::Notification { method } => Some((None, method.to_string())),
            Kind::Response { .. } => {
                // The client answering a request the server sent it.
                if !shared {
                    self.write_upstream(&msg).await;
                }
                return;
            }
            Kind::Invalid => None,
        };
        let Some((id, method)) = kind else { return };
        let Some(id) = id else {
            // Notifications.
            match method.as_str() {
                "notifications/initialized" => {}
                "notifications/cancelled" => {
                    let target = msg.pointer("/params/requestId").cloned();
                    if let Some(target) = target {
                        if let Some(uid) = self.by_client.get(&(conn, jsonrpc::id_key(&target))).copied() {
                            msg["params"]["requestId"] = json!(uid);
                            self.write_upstream(&msg).await;
                        }
                    }
                }
                "notifications/roots/list_changed" | "notifications/progress" if shared => {}
                _ => self.write_upstream(&msg).await,
            }
            return;
        };
        self.requests += 1;
        if shared {
            match method.as_str() {
                "initialize" => {
                    let err = jsonrpc::error(id, jsonrpc::INVALID_REQUEST, "already initialized");
                    self.send_to(conn, &err);
                    return;
                }
                // One session must not change logging for everyone.
                "logging/setLevel" => {
                    self.send_to(conn, &jsonrpc::result(id, json!({})));
                    return;
                }
                "resources/subscribe" | "resources/unsubscribe" => {
                    let uri = msg.pointer("/params/uri").and_then(Value::as_str).unwrap_or_default().to_string();
                    let subscribe = method == "resources/subscribe";
                    let set = self.subs.entry(uri.clone()).or_default();
                    let forward = if subscribe {
                        let first = set.is_empty();
                        set.insert(conn);
                        first
                    } else {
                        let removed = set.remove(&conn);
                        let now_empty = set.is_empty();
                        if now_empty {
                            self.subs.remove(&uri);
                        }
                        removed && now_empty
                    };
                    if !forward {
                        self.send_to(conn, &jsonrpc::result(id, json!({})));
                        return;
                    }
                }
                "tools/list" | "prompts/list" | "resources/list" | "resources/templates/list" => {
                    let key = cache_key(&method, msg.get("params"));
                    if let Some(hit) = self.cache.get(&key).cloned() {
                        self.cache_hits += 1;
                        self.send_to(conn, &jsonrpc::result(id, hit));
                        return;
                    }
                    self.forward(conn, id, msg, Some(key)).await;
                    return;
                }
                _ => {}
            }
        }
        self.forward(conn, id, msg, None).await;
    }

    async fn forward(&mut self, conn: u64, client_id: Value, mut msg: Value, cache_key: Option<String>) {
        let uid = self.alloc_id();
        let mut progress_token = None;
        if let Some(token) = msg.pointer("/params/_meta/progressToken").cloned() {
            let ours = format!("hangar-{uid}");
            msg["params"]["_meta"]["progressToken"] = json!(ours);
            self.progress.insert(ours.clone(), (conn, token));
            progress_token = Some(ours);
        }
        msg["id"] = json!(uid);
        self.by_client.insert((conn, jsonrpc::id_key(&client_id)), uid);
        self.pending.insert(uid, Pending { conn, client_id, progress_token, cache_key });
        self.write_upstream(&msg).await;
    }

    // ------------------------------------------------------- server → client

    async fn on_upstream_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Ok(mut msg) = serde_json::from_str::<Value>(line) else {
            self.push_stderr(format!("[stdout, not JSON] {line}"));
            return;
        };
        match jsonrpc::classify(&msg) {
            Kind::Response { id } => {
                let Some(uid) = id.as_u64() else { return };
                if Some(uid) == self.init_upstream_id {
                    self.on_init_response(msg).await;
                    return;
                }
                let Some(p) = self.pending.remove(&uid) else { return };
                if let Some(t) = &p.progress_token {
                    self.progress.remove(t);
                }
                if p.conn == 0 {
                    return; // one of our own requests
                }
                self.by_client.remove(&(p.conn, jsonrpc::id_key(&p.client_id)));
                if let (Some(key), Some(result)) = (&p.cache_key, msg.get("result")) {
                    self.cache.insert(key.clone(), result.clone());
                }
                msg["id"] = p.client_id;
                self.send_to(p.conn, &msg);
            }
            Kind::Request { id, method } => {
                let id = id.clone();
                if method == "ping" {
                    self.write_upstream(&jsonrpc::result(id, json!({}))).await;
                } else if self.share.is_shared() {
                    let err = jsonrpc::error(id, jsonrpc::METHOD_NOT_FOUND, "this server is shared by several sessions; per-client requests are not available");
                    self.write_upstream(&err).await;
                } else if let Some(conn) = self.conns.keys().next().copied() {
                    self.send_to(conn, &msg);
                } else {
                    let err = jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, "no client attached");
                    self.write_upstream(&err).await;
                }
            }
            Kind::Notification { method } => {
                let method = method.to_string();
                match method.as_str() {
                    "notifications/progress" => {
                        let token = msg.pointer("/params/progressToken").and_then(Value::as_str).map(String::from);
                        if let Some((conn, original)) = token.and_then(|t| self.progress.get(&t).cloned()) {
                            msg["params"]["progressToken"] = original;
                            self.send_to(conn, &msg);
                        } else if !self.share.is_shared() {
                            self.broadcast(&msg);
                        }
                    }
                    "notifications/resources/updated" => {
                        if self.share.is_shared() {
                            let uri = msg.pointer("/params/uri").and_then(Value::as_str).unwrap_or_default();
                            let targets: Vec<u64> = self.subs.get(uri).map(|s| s.iter().copied().collect()).unwrap_or_default();
                            for conn in targets {
                                self.send_to(conn, &msg);
                            }
                        } else {
                            self.broadcast(&msg);
                        }
                    }
                    "notifications/cancelled" if self.share.is_shared() => {}
                    other => {
                        self.invalidate_for(other);
                        self.broadcast(&msg);
                    }
                }
            }
            Kind::Invalid => {}
        }
    }

    fn invalidate_for(&mut self, notification: &str) {
        let prefixes: &[&str] = match notification {
            "notifications/tools/list_changed" => &["tools/list"],
            "notifications/prompts/list_changed" => &["prompts/list"],
            "notifications/resources/list_changed" => &["resources/list", "resources/templates/list"],
            _ => return,
        };
        self.cache.retain(|k, _| !prefixes.iter().any(|p| k.starts_with(&format!("{p}|"))));
    }

    async fn on_init_response(&mut self, msg: Value) {
        self.init_upstream_id = None;
        self.init_deadline = None;
        let Some(result) = msg.get("result").cloned() else {
            let err = msg.get("error").map(|e| e.to_string()).unwrap_or_else(|| "bad initialize response".into());
            self.fail_start(format!("initialize failed: {err}")).await;
            return;
        };
        self.write_upstream(&jsonrpc::notification("notifications/initialized", None)).await;
        self.init_result = Some(result.clone());
        self.phase = Phase::Ready;
        let restarted = self.was_ready;
        self.was_ready = true;
        tracing::info!(upstream = self.id, name = %self.name, restarted, "server ready");
        for (conn, id) in std::mem::take(&mut self.waiting_init) {
            self.send_to(conn, &jsonrpc::result(id, result.clone()));
        }
        if restarted {
            // The new process may offer different tools; let sessions refresh.
            let caps = result.get("capabilities").cloned().unwrap_or(Value::Null);
            for (cap, note) in [("tools", "notifications/tools/list_changed"), ("prompts", "notifications/prompts/list_changed"), ("resources", "notifications/resources/list_changed")] {
                if caps.get(cap).is_some() {
                    self.broadcast(&jsonrpc::notification(note, None));
                }
            }
            for uri in self.subs.keys().cloned().collect::<Vec<_>>() {
                let uid = self.alloc_id();
                self.pending.insert(uid, Pending { conn: 0, client_id: Value::Null, progress_token: None, cache_key: None });
                self.write_upstream(&jsonrpc::request(json!(uid), "resources/subscribe", json!({"uri": uri}))).await;
            }
        }
        while let Some((conn, msg)) = self.queued.pop_front() {
            if self.conns.contains_key(&conn) {
                self.route_client(conn, msg).await;
            }
        }
    }

    // ---------------------------------------------------------- process

    async fn spawn_process(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let launch = if self.opts.fast_npx && !self.fast_failed {
            crate::launch::plan(&self.spec, &self.env, true)
        } else {
            crate::launch::as_configured(&self.spec)
        };
        let program = launch.program.clone();
        let mut cmd = Command::new(&program);
        cmd.args(&launch.args);
        self.launch = Some(launch);
        if self.cwd.is_dir() {
            cmd.current_dir(&self.cwd);
        }
        if !self.env.is_empty() {
            cmd.env_clear().envs(&self.env);
        }
        cmd.envs(&self.spec.env);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.fail_start(format!("could not start {}: {e}", program.display())).await;
                return;
            }
        };
        let pid = child.id().unwrap_or(0);
        let job = Job::new_kill_on_close().ok();
        if let Some(j) = &job {
            if let Err(e) = j.assign_pid(pid) {
                tracing::warn!(pid, "could not contain server in a job: {e}");
            }
        }
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let tx = self.ev_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::with_capacity(256 * 1024, stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf).into_owned();
                        if tx.send(ProcEvent::Line(generation, line)).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(ProcEvent::Eof(generation));
        });
        let ring = self.stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut r = ring.lock().unwrap();
                if r.len() >= STDERR_LINES {
                    r.pop_front();
                }
                r.push_back(line);
            }
        });
        self.proc_ = Some(Proc { child, stdin, job, pid });
        self.started_at_ms = Some(now_ms());
        let uid = self.alloc_id();
        self.init_upstream_id = Some(uid);
        self.init_deadline = Some(Instant::now() + self.opts.init_timeout);
        self.phase = Phase::Starting;
        let init = jsonrpc::request(json!(uid), "initialize", self.init_params.clone());
        self.write_upstream(&init).await;
        tracing::info!(upstream = self.id, name = %self.name, pid, "started server");
    }

    async fn fail_start(&mut self, reason: String) {
        if self.launch.as_ref().is_some_and(|l| l.fast) && !self.fast_failed {
            // The shortcut did not work for this server; never try it again
            // and start it exactly as configured, keeping waiting sessions.
            tracing::warn!(upstream = self.id, name = %self.name, "fast start failed ({reason}); using the configured command");
            self.fast_failed = true;
            self.kill_process().await;
            Box::pin(self.spawn_process()).await; // recursion is bounded: fast_failed is now set
            return;
        }
        tracing::warn!(upstream = self.id, name = %self.name, "{reason}");
        self.last_error = Some(reason.clone());
        self.kill_process().await;
        let msg = format!("{} could not start: {reason}", self.name);
        for (conn, id) in std::mem::take(&mut self.waiting_init) {
            self.send_to(conn, &jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, msg.clone()));
        }
        self.fail_queued(&msg);
        self.phase = Phase::Failed { retry_at: Instant::now() + FAILED_RETRY };
        self.init_upstream_id = None;
        self.init_deadline = None;
        if self.conns.is_empty() {
            self.done = true;
        }
    }

    fn fail_queued(&mut self, message: &str) {
        for (conn, msg) in std::mem::take(&mut self.queued) {
            if let Kind::Request { id, .. } = jsonrpc::classify(&msg) {
                let err = jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, message.to_string());
                self.send_to(conn, &err);
            }
        }
    }

    async fn on_exit(&mut self) {
        let exit = self.kill_process().await;
        if matches!(self.phase, Phase::Stopping) {
            return;
        }
        let reason = match exit {
            Some(code) => format!("server exited with code {code}"),
            None => "server exited".into(),
        };
        tracing::warn!(upstream = self.id, name = %self.name, "{reason}");
        self.last_error = Some(reason.clone());
        let pending: Vec<Pending> = self.pending.drain().map(|(_, p)| p).collect();
        for p in pending {
            if p.conn != 0 {
                let err = jsonrpc::error(p.client_id, jsonrpc::SERVER_UNAVAILABLE, format!("{} stopped unexpectedly ({reason}); hangar is restarting it", self.name));
                self.send_to(p.conn, &err);
            }
        }
        self.by_client.clear();
        self.progress.clear();
        self.cache.clear();
        if matches!(self.phase, Phase::Starting) {
            self.fail_start(reason).await;
            return;
        }
        if self.conns.is_empty() {
            self.done = true;
            return;
        }
        let now = Instant::now();
        self.crashes.push_back(now);
        while self.crashes.front().is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW) {
            self.crashes.pop_front();
        }
        self.restarts += 1;
        if self.crashes.len() >= MAX_CRASHES {
            let msg = format!("{} crashed {} times in a minute", self.name, self.crashes.len());
            self.last_error = Some(msg.clone());
            self.fail_queued(&msg);
            self.phase = Phase::Failed { retry_at: now + FAILED_RETRY };
            return;
        }
        let backoff = Duration::from_millis(250 * (1u64 << (self.crashes.len() as u32 - 1).min(5)));
        self.phase = Phase::Backoff(now + backoff);
    }

    /// Closes stdin, gives the server a moment to exit, then kills its tree.
    async fn stop_process(&mut self) {
        self.phase = Phase::Stopping;
        if let Some(mut p) = self.proc_.take() {
            let _ = p.stdin.shutdown().await;
            drop(p.stdin);
            if tokio::time::timeout(Duration::from_secs(2), p.child.wait()).await.is_err() {
                if let Some(job) = &p.job {
                    job.terminate();
                }
                let _ = p.child.start_kill();
                let _ = p.child.wait().await;
            }
            if let Some(job) = p.job.take() {
                job.terminate();
            }
        }
    }

    /// Makes sure the current process (if any) is gone. Returns its exit code.
    async fn kill_process(&mut self) -> Option<i32> {
        let mut p = self.proc_.take()?;
        let code = match tokio::time::timeout(Duration::from_millis(500), p.child.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => None,
        };
        if let Some(job) = p.job.take() {
            job.terminate();
        }
        let _ = p.child.start_kill();
        let _ = p.child.wait().await;
        code
    }

    async fn write_upstream(&mut self, msg: &Value) {
        let Some(p) = self.proc_.as_mut() else { return };
        let line = jsonrpc::to_line(msg);
        if p.stdin.write_all(line.as_bytes()).await.is_err() || p.stdin.flush().await.is_err() {
            // The reader will report EOF; restart logic lives there.
            tracing::debug!(upstream = self.id, "write to server failed");
        }
    }

    fn send_to(&self, conn: u64, msg: &Value) {
        if let Some(c) = self.conns.get(&conn) {
            let _ = c.tx.send(jsonrpc::to_line(msg));
        }
    }

    fn broadcast(&self, msg: &Value) {
        let line = jsonrpc::to_line(msg);
        for c in self.conns.values() {
            let _ = c.tx.send(line.clone());
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn push_stderr(&self, line: String) {
        let mut r = self.stderr.lock().unwrap();
        if r.len() >= STDERR_LINES {
            r.pop_front();
        }
        r.push_back(line);
    }
}

fn cache_key(method: &str, params: Option<&Value>) -> String {
    let mut p = params.cloned().unwrap_or(Value::Null);
    if let Some(obj) = p.as_object_mut() {
        obj.remove("_meta");
    }
    format!("{method}|{p}")
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_ignores_meta() {
        let a = cache_key("tools/list", Some(&json!({"_meta": {"progressToken": 1}, "cursor": "x"})));
        let b = cache_key("tools/list", Some(&json!({"cursor": "x"})));
        assert_eq!(a, b);
        assert_ne!(a, cache_key("tools/list", None));
    }

    #[test]
    fn shared_servers_never_see_client_capabilities() {
        let client = json!({"protocolVersion": "2025-06-18", "capabilities": {"roots": {}, "sampling": {}}, "clientInfo": {"name": "x"}});
        assert_eq!(upstream_init_params(&client, Share::Global)["capabilities"], json!({}));
        assert_eq!(upstream_init_params(&client, Share::Session)["capabilities"]["roots"], json!({}));
        assert_eq!(upstream_init_params(&client, Share::Project)["clientInfo"]["name"], "x");
    }
}
