//! One pooled stdio MCP server process and everything needed to share it.
//!
//! An upstream is an actor: one task owns the process and all routing state,
//! and the rest of the daemon talks to it through [`Cmd`]s.
//!
//! Routing rules for shared servers:
//! * each session's request id is replaced with one unique on the upstream
//!   and swapped back on the response;
//! * `_meta.progressToken` and `notifications/cancelled` are remapped the
//!   same way, so sessions never see each other's progress or cancel each
//!   other's requests;
//! * `resources/subscribe` is reference counted (and rolled back if the
//!   server refuses it); updates go only to subscribers;
//! * `tools/list`, `prompts/list`, `resources/list` and
//!   `resources/templates/list` are cached until the server says they changed;
//! * the server is initialized once, without per-client capabilities (roots,
//!   sampling, elicitation), so it never asks one session a question meant
//!   for another.
//!
//! A `session`-scoped upstream has exactly one session and passes everything
//! through, including requests from the server to the client.

use super::process::{self, Ring};
use super::{PoolKey, PoolOptions};
use crate::jsonrpc::{self, Kind};
use crate::launch::{self, FastPath, Launch};
use crate::policy::Share;
use crate::util::{now_ms, redact_command};
use hangar_proto::ServerSpec;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};

const MAX_QUEUED: usize = 10_000;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const MAX_CRASHES: usize = 5;
pub(crate) const FAILED_RETRY: Duration = Duration::from_secs(30);

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

impl ConnInfo {
    pub fn summary(&self) -> SessionRef {
        SessionRef {
            conn: self.conn,
            name: self.name.clone(),
            session: self.session.clone(),
            client_pid: self.client_pid,
            shim_pid: self.shim_pid,
            attached_at_ms: self.attached_at_ms,
        }
    }
}

#[derive(Debug)]
pub struct Rejected;

pub enum Cmd {
    /// `init_id` is the id of the session's `initialize` request, or `None`
    /// when a shim reconnects and the session is already initialized.
    /// `init_params` are the session's own initialize params (multi-session
    /// HTTP servers initialize every session separately).
    Attach {
        info: ConnInfo,
        init_id: Option<Value>,
        init_params: Value,
        reply: oneshot::Sender<Result<(), Rejected>>,
    },
    Client {
        conn: u64,
        msg: Value,
    },
    Detach {
        conn: u64,
    },
    Stop {
        reply: Option<oneshot::Sender<()>>,
    },
    Stats {
        reply: oneshot::Sender<UpstreamStats>,
    },
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
    /// The configured command, with likely secrets masked.
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
    /// The command actually running (differs on the npx fast path), masked.
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

    /// Stats, or `None` if the actor does not answer within a second.
    pub async fn stats(&self) -> Option<UpstreamStats> {
        let (reply, rx) = oneshot::channel();
        if !self.send(Cmd::Stats { reply }) {
            return None;
        }
        tokio::time::timeout(Duration::from_secs(1), rx).await.ok()?.ok()
    }
}

pub(crate) fn display(command: &str, args: &[String]) -> String {
    redact_command(&std::iter::once(command.to_string()).chain(args.iter().cloned()).collect::<Vec<_>>())
}

pub(crate) fn launch_display(l: &Launch) -> String {
    display(&l.program.display().to_string(), &l.args)
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
    pub on_exit: Weak<super::OnExit>,
    /// The pool's running total for this server name.
    pub served: Arc<std::sync::atomic::AtomicU64>,
}

pub fn start(spawn: Spawn) -> Upstream {
    let (tx, rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let handle = Upstream { id: spawn.id, key: spawn.key.clone(), tx };
    let init_params = upstream_init_params(&spawn.client_init, spawn.share);
    let actor = Actor {
        init_params,
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
        s: spawn,
    };
    tokio::spawn(actor.run());
    handle
}

/// What the upstream sees as its client's `initialize` params.
fn upstream_init_params(client: &Value, share: Share) -> Value {
    let mut params = client.clone();
    if share.is_shared()
        && let Some(obj) = params.as_object_mut()
    {
        // Per-client features cannot be shared, so the server never sees them.
        obj.insert("capabilities".into(), json!({}));
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

enum ProcEvent {
    Line(u64, String),
    Eof(u64),
}

#[derive(Clone)]
enum PendingKind {
    Normal,
    /// A forwarded subscribe; roll it back if the server refuses.
    Subscribe(String),
    /// Our own request (re-subscribe, unsubscribe); the answer is dropped.
    Internal,
}

struct Pending {
    conn: u64,
    client_id: Value,
    progress_token: Option<String>,
    cache_key: Option<String>,
    kind: PendingKind,
}

struct Actor {
    s: Spawn,
    init_params: Value,
    rx: mpsc::UnboundedReceiver<Cmd>,
    ev_tx: mpsc::UnboundedSender<ProcEvent>,
    ev_rx: mpsc::UnboundedReceiver<ProcEvent>,
    phase: Phase,
    proc_: Option<process::Started>,
    /// Bumped whenever a process starts or is killed, so late events from an
    /// old process are recognized and ignored.
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
    stderr: Ring,
    done: bool,
    launch: Option<Launch>,
    /// The npx fast path failed once; use the configured command from now on.
    fast_failed: bool,
}

// Lives for one loop iteration on the stack; boxing would only add an allocation.
#[allow(clippy::large_enum_variant)]
enum Wake {
    Cmd(Option<Cmd>),
    Proc(Option<ProcEvent>),
    Timer,
}

impl Actor {
    async fn run(mut self) {
        tracing::debug!(upstream = self.s.id, name = %self.s.name, scope = %self.s.key.scope, "upstream created");
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
        if let Some(cb) = self.s.on_exit.upgrade() {
            cb(self.s.id, &self.s.key);
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
        tracing::debug!(upstream = self.s.id, name = %self.s.name, "upstream finished");
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
        if self.s.share.is_shared() { self.s.opts.idle_timeout } else { self.s.opts.session_grace }
    }

    async fn on_timer(&mut self) {
        let now = Instant::now();
        if let Phase::Starting = self.phase
            && self.init_deadline.is_some_and(|t| now >= t)
        {
            self.fail_start(format!("no answer to initialize after {}s", self.s.opts.init_timeout.as_secs())).await;
            return;
        }
        if let Phase::Backoff(t) = self.phase
            && now >= t
        {
            self.spawn_process().await;
            return;
        }
        if self.conns.is_empty() && self.idle_since.is_some_and(|s| now >= s + self.idle_limit()) {
            tracing::info!(upstream = self.s.id, name = %self.s.name, "stopping idle server");
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
                if let Phase::Failed { retry_at } = self.phase
                    && Instant::now() >= retry_at
                {
                    self.spawn_process().await;
                }
                if let Some(id) = init_id {
                    match (&self.phase, &self.init_result) {
                        (Phase::Ready, Some(result)) => {
                            let msg = jsonrpc::result(id, result.clone());
                            self.send_to(conn, &msg);
                        }
                        // Still failing: say why right away instead of hanging.
                        (Phase::Failed { .. }, _) => {
                            let msg = jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, self.unavailable());
                            self.send_to(conn, &msg);
                        }
                        _ => self.waiting_init.push((conn, id)),
                    }
                }
                let _ = reply.send(Ok(()));
            }
            Cmd::Client { conn, msg } => {
                if self.conns.contains_key(&conn) {
                    self.on_client(conn, msg).await;
                }
            }
            Cmd::Detach { conn } => self.detach(conn),
            Cmd::Stop { reply } => {
                self.fail_pending(&format!("{} was stopped", self.s.name));
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

    fn unavailable(&self) -> String {
        let reason = self.last_error.clone().unwrap_or_else(|| "it failed to start".into());
        format!("{} is unavailable: {reason}", self.s.name)
    }

    fn snapshot(&self) -> UpstreamStats {
        let mut sessions: Vec<SessionRef> = self.conns.values().map(ConnInfo::summary).collect();
        sessions.sort_by_key(|s| s.conn);
        UpstreamStats {
            id: self.s.id,
            name: self.s.name.clone(),
            share: self.s.share,
            scope: self.s.key.scope.clone(),
            command: display(&self.s.spec.command, &self.s.spec.args),
            pid: self.proc_.as_ref().map(|p| p.pid),
            state: self.phase.label().to_string(),
            sessions,
            started_at_ms: self.started_at_ms,
            restarts: self.restarts,
            requests: self.requests,
            cache_hits: self.cache_hits,
            last_error: self.last_error.clone(),
            stderr_tail: process::tail(&self.stderr, 20),
            launch: self.launch.as_ref().map(launch_display),
            fast_path: self.launch.as_ref().is_some_and(|l| l.fast),
        }
    }

    fn detach(&mut self, conn: u64) {
        if self.conns.remove(&conn).is_none() {
            return;
        }
        self.waiting_init.retain(|(c, _)| *c != conn);
        self.queued.retain(|(c, _)| *c != conn);
        // Tell the server to stop work nobody is waiting for.
        let abandoned: Vec<u64> = self.pending.iter().filter(|(_, p)| p.conn == conn).map(|(id, _)| *id).collect();
        for uid in abandoned {
            self.forget_request(uid);
            let cancel = jsonrpc::notification("notifications/cancelled", Some(json!({"requestId": uid, "reason": "client disconnected"})));
            self.write_upstream(&cancel);
        }
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
                self.internal_request("resources/unsubscribe", json!({"uri": uri}));
            }
        }
        if self.conns.is_empty() {
            self.idle_since = Some(Instant::now());
        }
    }

    fn forget_request(&mut self, uid: u64) -> Option<Pending> {
        let p = self.pending.remove(&uid)?;
        if let Some(t) = &p.progress_token {
            self.progress.remove(t);
        }
        self.by_client.remove(&(p.conn, jsonrpc::id_key(&p.client_id)));
        Some(p)
    }

    fn internal_request(&mut self, method: &str, params: Value) {
        let uid = self.alloc_id();
        self.pending.insert(uid, Pending { conn: 0, client_id: Value::Null, progress_token: None, cache_key: None, kind: PendingKind::Internal });
        self.write_upstream(&jsonrpc::request(json!(uid), method, params));
    }

    // ------------------------------------------------------- client → server

    async fn on_client(&mut self, conn: u64, msg: Value) {
        match self.phase {
            Phase::Ready => {}
            Phase::Failed { retry_at } if Instant::now() < retry_at => {
                if let Kind::Request { id, .. } = jsonrpc::classify(&msg) {
                    let err = jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, self.unavailable());
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
        self.route_client(conn, msg);
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

    fn route_client(&mut self, conn: u64, mut msg: Value) {
        let shared = self.s.share.is_shared();
        let (id, method) = match jsonrpc::classify(&msg) {
            Kind::Request { id, method } => (Some(id.clone()), method.to_string()),
            Kind::Notification { method } => (None, method.to_string()),
            Kind::Response { .. } => {
                // The client answering a request the server sent it.
                if !shared {
                    self.write_upstream(&msg);
                }
                return;
            }
            Kind::Invalid => return,
        };
        let Some(id) = id else {
            match method.as_str() {
                "notifications/initialized" => {}
                "notifications/cancelled" => {
                    let target = msg.pointer("/params/requestId").cloned();
                    if let Some(uid) = target.and_then(|t| self.by_client.get(&(conn, jsonrpc::id_key(&t))).copied()) {
                        msg["params"]["requestId"] = json!(uid);
                        self.write_upstream(&msg);
                        // Servers do not answer cancelled requests.
                        self.forget_request(uid);
                    }
                }
                "notifications/roots/list_changed" | "notifications/progress" if shared => {}
                _ => self.write_upstream(&msg),
            }
            return;
        };
        self.requests += 1;
        self.s.served.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut kind = PendingKind::Normal;
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
                    if subscribe {
                        kind = PendingKind::Subscribe(uri);
                    }
                }
                "tools/list" | "prompts/list" | "resources/list" | "resources/templates/list" => {
                    let key = cache_key(&method, msg.get("params"));
                    if let Some(hit) = self.cache.get(&key).cloned() {
                        self.cache_hits += 1;
                        self.send_to(conn, &jsonrpc::result(id, hit));
                        return;
                    }
                    self.forward(conn, id, msg, Some(key), kind);
                    return;
                }
                _ => {}
            }
        }
        self.forward(conn, id, msg, None, kind);
    }

    fn forward(&mut self, conn: u64, client_id: Value, mut msg: Value, cache_key: Option<String>, kind: PendingKind) {
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
        self.pending.insert(uid, Pending { conn, client_id, progress_token, cache_key, kind });
        self.write_upstream(&msg);
    }

    // ------------------------------------------------------- server → client

    async fn on_upstream_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Ok(mut msg) = serde_json::from_str::<Value>(line) else {
            process::push(&self.stderr, format!("[stdout, not JSON] {line}"));
            return;
        };
        match jsonrpc::classify(&msg) {
            Kind::Response { id } => {
                let Some(uid) = id.as_u64() else { return };
                if Some(uid) == self.init_upstream_id {
                    self.on_init_response(msg).await;
                    return;
                }
                let Some(p) = self.forget_request(uid) else { return };
                let failed = msg.get("error").is_some();
                match &p.kind {
                    PendingKind::Internal => return,
                    PendingKind::Subscribe(uri) if failed => {
                        if let Some(set) = self.subs.get_mut(uri) {
                            set.remove(&p.conn);
                            if set.is_empty() {
                                self.subs.remove(uri);
                            }
                        }
                    }
                    _ => {}
                }
                if let (Some(key), Some(result)) = (&p.cache_key, msg.get("result")) {
                    self.cache.insert(key.clone(), result.clone());
                }
                msg["id"] = p.client_id;
                self.send_to(p.conn, &msg);
            }
            Kind::Request { id, method } => {
                let id = id.clone();
                if method == "ping" {
                    self.write_upstream(&jsonrpc::result(id, json!({})));
                } else if self.s.share.is_shared() {
                    let err = jsonrpc::error(id, jsonrpc::METHOD_NOT_FOUND, "this server is shared by several sessions; per-client requests are not available");
                    self.write_upstream(&err);
                } else if let Some(conn) = self.conns.keys().next().copied() {
                    self.send_to(conn, &msg);
                } else {
                    let err = jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, "no client attached");
                    self.write_upstream(&err);
                }
            }
            Kind::Notification { method } => {
                let method = method.to_string();
                let shared = self.s.share.is_shared();
                match method.as_str() {
                    "notifications/progress" => {
                        let token = msg.pointer("/params/progressToken").and_then(Value::as_str).map(String::from);
                        if let Some((conn, original)) = token.and_then(|t| self.progress.get(&t).cloned()) {
                            msg["params"]["progressToken"] = original;
                            self.send_to(conn, &msg);
                        } else if !shared {
                            self.broadcast(&msg);
                        }
                    }
                    "notifications/resources/updated" if shared => {
                        let uri = msg.pointer("/params/uri").and_then(Value::as_str).unwrap_or_default();
                        let targets: Vec<u64> = self.subs.get(uri).map(|s| s.iter().copied().collect()).unwrap_or_default();
                        for conn in targets {
                            self.send_to(conn, &msg);
                        }
                    }
                    "notifications/cancelled" if shared => {}
                    // A shared server's log lines can quote one session's
                    // request; they are not passed on to every session.
                    "notifications/message" if shared => {}
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
        self.write_upstream(&jsonrpc::notification("notifications/initialized", None));
        if self.launch.as_ref().is_some_and(|l| !l.fast) {
            launch::note_npx_ran(self.s.opts.state_dir.as_deref(), &self.s.spec);
        }
        self.init_result = Some(result.clone());
        self.phase = Phase::Ready;
        let restarted = self.was_ready;
        self.was_ready = true;
        tracing::info!(upstream = self.s.id, name = %self.s.name, restarted, "server ready");
        for (conn, id) in std::mem::take(&mut self.waiting_init) {
            self.send_to(conn, &jsonrpc::result(id, result.clone()));
        }
        if restarted {
            // The new process may offer different tools; let sessions refresh.
            let caps = result.get("capabilities").cloned().unwrap_or(Value::Null);
            for (cap, note) in [
                ("tools", "notifications/tools/list_changed"),
                ("prompts", "notifications/prompts/list_changed"),
                ("resources", "notifications/resources/list_changed"),
            ] {
                if caps.get(cap).is_some() {
                    self.broadcast(&jsonrpc::notification(note, None));
                }
            }
            for uri in self.subs.keys().cloned().collect::<Vec<_>>() {
                self.internal_request("resources/subscribe", json!({"uri": uri}));
            }
        }
        while let Some((conn, msg)) = self.queued.pop_front() {
            if self.conns.contains_key(&conn) {
                self.route_client(conn, msg);
            }
        }
    }

    // ---------------------------------------------------------- process

    async fn spawn_process(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let fast = FastPath { enabled: self.s.opts.fast_npx && !self.fast_failed, state_dir: self.s.opts.state_dir.as_deref() };
        let launch = launch::plan(&self.s.spec, &self.s.env, fast);
        let opts =
            process::Options { launch: &launch, extra_args: &[], cwd: &self.s.cwd, env: &self.s.env, declared: &self.s.spec.env, stdin: true, stdout: true };
        let started = process::start(opts, &self.stderr);
        self.launch = Some(launch);
        let mut started = match started {
            Ok(s) => s,
            Err(e) => {
                let program = self.launch.as_ref().map(|l| l.program.display().to_string()).unwrap_or_default();
                self.fail_start(format!("could not start {program}: {e}")).await;
                return;
            }
        };
        let stdout = started.stdout.take().expect("piped stdout");
        let tx = self.ev_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::with_capacity(64 * 1024, stdout);
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
        let pid = started.pid;
        self.proc_ = Some(started);
        self.started_at_ms = Some(now_ms());
        let uid = self.alloc_id();
        self.init_upstream_id = Some(uid);
        self.init_deadline = Some(Instant::now() + self.s.opts.init_timeout);
        self.phase = Phase::Starting;
        self.write_upstream(&jsonrpc::request(json!(uid), "initialize", self.init_params.clone()));
        tracing::info!(upstream = self.s.id, name = %self.s.name, pid, "started server");
    }

    async fn fail_start(&mut self, reason: String) {
        self.kill_process().await;
        if self.launch.as_ref().is_some_and(|l| l.fast) && !self.fast_failed {
            // The shortcut did not work for this server; never try it again
            // and start it exactly as configured, keeping waiting sessions.
            tracing::warn!(upstream = self.s.id, name = %self.s.name, "fast start failed ({reason}); using the configured command");
            self.fast_failed = true;
            Box::pin(self.spawn_process()).await; // bounded: fast_failed is now set
            return;
        }
        tracing::warn!(upstream = self.s.id, name = %self.s.name, "{reason}");
        self.last_error = Some(last_line_of_stderr(&self.stderr).map_or(reason.clone(), |l| format!("{reason} ({l})")));
        let msg = self.unavailable();
        for (conn, id) in std::mem::take(&mut self.waiting_init) {
            self.send_to(conn, &jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, msg.clone()));
        }
        self.fail_queued(&msg);
        self.phase = Phase::Failed { retry_at: Instant::now() + FAILED_RETRY };
        self.init_upstream_id = None;
        self.init_deadline = None;
        // Keep the failure (and its reason) around for sessions that arrive
        // soon, and clean up after the idle limit if none do.
        if self.conns.is_empty() {
            self.idle_since = Some(Instant::now());
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

    /// Answers every request in flight with an error (the process is gone).
    fn fail_pending(&mut self, message: &str) {
        let pending: Vec<Pending> = self.pending.drain().map(|(_, p)| p).collect();
        for p in pending {
            if p.conn != 0 {
                self.send_to(p.conn, &jsonrpc::error(p.client_id, jsonrpc::SERVER_UNAVAILABLE, message.to_string()));
            }
        }
        self.by_client.clear();
        self.progress.clear();
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
        if matches!(self.phase, Phase::Starting) {
            self.fail_start(reason).await;
            return;
        }
        tracing::warn!(upstream = self.s.id, name = %self.s.name, "{reason}");
        self.last_error = Some(reason.clone());
        self.fail_pending(&format!("{} stopped unexpectedly ({reason}); hangar is restarting it", self.s.name));
        self.cache.clear();
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
            self.last_error = Some(format!("crashed {} times in a minute", self.crashes.len()));
            let msg = self.unavailable();
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
        self.generation += 1;
        if let Some(p) = self.proc_.take() {
            process::stop(p, Duration::from_secs(2)).await;
        }
    }

    /// Makes sure the current process (if any) is gone. Returns its exit code.
    async fn kill_process(&mut self) -> Option<i32> {
        self.generation += 1; // anything the old process still says is stale
        let mut p = self.proc_.take()?;
        let code = match tokio::time::timeout(Duration::from_millis(500), p.child.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => None,
        };
        process::stop(p, Duration::ZERO).await;
        code
    }

    fn write_upstream(&mut self, msg: &Value) {
        if let Some(stdin) = self.proc_.as_ref().and_then(|p| p.stdin.as_ref()) {
            let _ = stdin.send(jsonrpc::to_line(msg));
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
}

fn last_line_of_stderr(ring: &Ring) -> Option<String> {
    process::tail(ring, 1).pop().filter(|l| !l.trim().is_empty()).map(|l| l.chars().take(200).collect())
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
    fn displays_mask_secrets() {
        assert_eq!(display("npx", &["-y".into(), "srv".into(), "--api-key".into(), "sk-1".into()]), "npx -y srv --api-key ***");
    }
}
