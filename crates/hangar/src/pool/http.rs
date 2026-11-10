//! Servers that natively serve many clients over Streamable HTTP.
//!
//! Some servers keep per-client state but can also run in HTTP mode, where
//! each HTTP session gets its own state inside one process. Playwright MCP
//! with `--isolated` is the important one: each session gets its own browser
//! context (own pages, cookies and storage) in one shared browser.
//!
//! Hangar starts one such process and opens one MCP HTTP session per agent
//! session. Ids, progress tokens and server-to-client requests are per HTTP
//! session already, so messages pass through unchanged.

use super::upstream::{now_ms, Cmd, Rejected, SessionRef, Upstream, UpstreamStats};
use super::{PoolKey, PoolOptions};
use crate::jsonrpc::{self, Kind};
use crate::policy::Share;
use crate::procs::Job;
use futures_util::StreamExt;
use hangar_proto::ServerSpec;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};

const STDERR_LINES: usize = 200;
const MAX_CRASHES: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

/// Extra arguments that switch a server into multi-client HTTP mode.
/// `{port}` is replaced with a free loopback port.
pub fn native_args(package: Option<&str>, spec: &ServerSpec) -> Option<Vec<String>> {
    let has = |flag: &str| spec.args.iter().any(|a| a == flag || a.starts_with(&format!("{flag}=")));
    match package? {
        "@playwright/mcp" => {
            let conflicting = ["--port", "--host", "--extension", "--cdp-endpoint", "--endpoint", "--shared-browser-context"];
            if has("--isolated") && !conflicting.iter().any(|f| has(f)) {
                Some(vec!["--port".into(), "{port}".into(), "--host".into(), "127.0.0.1".into()])
            } else {
                None
            }
        }
        _ => None,
    }
}

pub struct HttpSpawn {
    pub id: u64,
    pub key: PoolKey,
    pub name: String,
    pub spec: ServerSpec,
    pub extra_args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub opts: PoolOptions,
    pub on_exit: Weak<dyn Fn(u64, &PoolKey) + Send + Sync>,
}

/// Where the server can be reached: (process generation, base URL).
type Endpoint = Option<(u64, String)>;

pub fn start(spawn: HttpSpawn) -> Upstream {
    let (tx, rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (url_tx, _) = watch::channel::<Endpoint>(None);
    let handle = Upstream::from_parts(spawn.id, spawn.key.clone(), tx);
    let http = reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(5)).build().expect("http client");
    let actor = HttpActor {
        spawn,
        http,
        rx,
        ev_tx,
        ev_rx,
        url_tx,
        sessions: HashMap::new(),
        generation: 0,
        state: State::Starting,
        kill: None,
        job: None,
        pid: None,
        port: 0,
        port_deadline: None,
        crashes: VecDeque::new(),
        backoff_until: None,
        idle_since: None,
        started_at_ms: None,
        restarts: 0,
        requests: 0,
        last_error: None,
        stderr: Arc::new(Mutex::new(VecDeque::new())),
        launch: None,
        fast_failed: false,
        done: false,
    };
    tokio::spawn(actor.run());
    handle
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Starting,
    Ready,
    Restarting,
    Failed,
    Stopping,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Ready => "ready",
            State::Restarting => "restarting",
            State::Failed => "failed",
            State::Stopping => "stopping",
        }
    }
}

enum Event {
    PortReady(u64),
    PortTimeout(u64),
    Exited(u64, Option<i32>),
}

struct SessionHandle {
    info: SessionRef,
    tx: mpsc::UnboundedSender<Value>,
}

struct HttpActor {
    spawn: HttpSpawn,
    http: reqwest::Client,
    rx: mpsc::UnboundedReceiver<Cmd>,
    ev_tx: mpsc::UnboundedSender<Event>,
    ev_rx: mpsc::UnboundedReceiver<Event>,
    url_tx: watch::Sender<Endpoint>,
    sessions: HashMap<u64, SessionHandle>,
    generation: u64,
    state: State,
    kill: Option<oneshot::Sender<()>>,
    job: Option<Job>,
    pid: Option<u32>,
    port: u16,
    port_deadline: Option<Instant>,
    crashes: VecDeque<Instant>,
    backoff_until: Option<Instant>,
    idle_since: Option<Instant>,
    started_at_ms: Option<u64>,
    restarts: u64,
    requests: u64,
    last_error: Option<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    launch: Option<crate::launch::Launch>,
    fast_failed: bool,
    done: bool,
}

enum Wake {
    Cmd(Option<Cmd>),
    Ev(Option<Event>),
    Timer,
}

impl HttpActor {
    async fn run(mut self) {
        self.spawn_process();
        while !self.done {
            let deadline = self.next_deadline();
            let wake = tokio::select! {
                c = self.rx.recv() => Wake::Cmd(c),
                e = self.ev_rx.recv() => Wake::Ev(e),
                _ = async { match deadline { Some(t) => tokio::time::sleep_until(t.into()).await, None => std::future::pending().await } } => Wake::Timer,
            };
            match wake {
                Wake::Cmd(None) => {
                    self.stop_process().await;
                    self.done = true;
                }
                Wake::Cmd(Some(cmd)) => self.on_cmd(cmd).await,
                Wake::Ev(Some(ev)) => self.on_event(ev),
                Wake::Ev(None) => {}
                Wake::Timer => self.on_timer().await,
            }
        }
        let _ = self.url_tx.send(None);
        if let Some(cb) = self.spawn.on_exit.upgrade() {
            cb(self.spawn.id, &self.spawn.key);
        }
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
    }

    fn next_deadline(&self) -> Option<Instant> {
        [self.port_deadline, self.backoff_until, self.idle_since.map(|s| s + self.spawn.opts.idle_timeout)].into_iter().flatten().min()
    }

    async fn on_timer(&mut self) {
        let now = Instant::now();
        if self.backoff_until.is_some_and(|t| now >= t) {
            self.backoff_until = None;
            self.spawn_process();
        }
        if self.sessions.is_empty() && self.idle_since.is_some_and(|s| now >= s + self.spawn.opts.idle_timeout) {
            tracing::info!(upstream = self.spawn.id, name = %self.spawn.name, "stopping idle server");
            self.stop_process().await;
            self.done = true;
        }
    }

    async fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Attach { info, init_id, init_params, reply } => {
                if self.state == State::Stopping || self.done {
                    let _ = reply.send(Err(Rejected));
                    return;
                }
                self.idle_since = None;
                let (tx, rx) = mpsc::unbounded_channel();
                let init = json!({"jsonrpc": "2.0", "id": init_id.clone().unwrap_or(json!("hangar-init")), "method": "initialize", "params": init_params});
                let session = SessionTask {
                    http: self.http.clone(),
                    url: self.url_tx.subscribe(),
                    to_client: info.tx.clone(),
                    init,
                    respond_init: init_id.is_some(),
                    rx,
                };
                tokio::spawn(session.run());
                let sref = SessionRef {
                    conn: info.conn,
                    name: info.name.clone(),
                    session: info.session.clone(),
                    client_pid: info.client_pid,
                    shim_pid: info.shim_pid,
                    attached_at_ms: info.attached_at_ms,
                };
                drop(info);
                self.sessions.insert(sref.conn, SessionHandle { info: sref, tx });
                if self.state == State::Failed {
                    self.spawn_process();
                }
                let _ = reply.send(Ok(()));
            }
            Cmd::Client { conn, msg } => {
                if let Some(s) = self.sessions.get(&conn) {
                    if matches!(jsonrpc::classify(&msg), Kind::Request { .. }) {
                        self.requests += 1;
                    }
                    let _ = s.tx.send(msg);
                }
            }
            Cmd::Detach { conn } => {
                self.sessions.remove(&conn); // the task sees its channel close and ends the HTTP session
                if self.sessions.is_empty() {
                    self.idle_since = Some(Instant::now());
                }
            }
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

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::PortReady(g) if g == self.generation => {
                self.port_deadline = None;
                self.state = State::Ready;
                let url = format!("http://127.0.0.1:{}/mcp", self.port);
                tracing::info!(upstream = self.spawn.id, name = %self.spawn.name, %url, "multi-session server ready");
                let _ = self.url_tx.send(Some((g, url)));
            }
            Event::PortTimeout(g) if g == self.generation => {
                self.fail("did not start listening in time".into());
            }
            Event::Exited(g, code) if g == self.generation => {
                self.kill = None;
                self.job = None;
                self.pid = None;
                if self.state == State::Stopping {
                    return;
                }
                let reason = code.map(|c| format!("server exited with code {c}")).unwrap_or_else(|| "server exited".into());
                if self.state == State::Starting {
                    self.fail(reason);
                    return;
                }
                tracing::warn!(upstream = self.spawn.id, name = %self.spawn.name, "{reason}");
                self.last_error = Some(reason);
                let _ = self.url_tx.send(None);
                if self.sessions.is_empty() {
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
                    self.state = State::Failed;
                    return;
                }
                self.state = State::Restarting;
                self.backoff_until = Some(now + Duration::from_millis(250 * (1 << (self.crashes.len() - 1).min(5))));
            }
            _ => {}
        }
    }

    fn fail(&mut self, reason: String) {
        if let Some(k) = self.kill.take() {
            let _ = k.send(());
        }
        self.job = None;
        self.port_deadline = None;
        if self.launch.as_ref().is_some_and(|l| l.fast) && !self.fast_failed {
            tracing::warn!(upstream = self.spawn.id, "fast start failed ({reason}); using the configured command");
            self.fast_failed = true;
            self.spawn_process();
            return;
        }
        tracing::warn!(upstream = self.spawn.id, name = %self.spawn.name, "{reason}");
        self.last_error = Some(reason);
        self.state = State::Failed;
        let _ = self.url_tx.send(None);
        if self.sessions.is_empty() {
            self.done = true;
        }
    }

    fn spawn_process(&mut self) {
        self.generation += 1;
        let g = self.generation;
        let port = match std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()) {
            Ok(a) => a.port(),
            Err(e) => {
                self.fail(format!("no free port: {e}"));
                return;
            }
        };
        self.port = port;
        let launch = if self.spawn.opts.fast_npx && !self.fast_failed {
            crate::launch::plan(&self.spawn.spec, &self.spawn.env, true)
        } else {
            crate::launch::as_configured(&self.spawn.spec)
        };
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args).args(self.spawn.extra_args.iter().map(|a| a.replace("{port}", &port.to_string())));
        if self.spawn.cwd.is_dir() {
            cmd.current_dir(&self.spawn.cwd);
        }
        if !self.spawn.env.is_empty() {
            cmd.env_clear().envs(&self.spawn.env);
        }
        cmd.envs(&self.spawn.spec.env);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.launch = Some(launch.clone());
                self.fail(format!("could not start {}: {e}", launch.program.display()));
                return;
            }
        };
        self.launch = Some(launch);
        let pid = child.id().unwrap_or(0);
        let job = Job::new_kill_on_close().ok();
        if let Some(j) = &job {
            let _ = j.assign_pid(pid);
        }
        self.job = job;
        self.pid = Some(pid);
        self.started_at_ms = Some(now_ms());
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>), child.stderr.take().map(|s| Box::new(s) as _)].into_iter().flatten() {
            let ring = self.stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut r = ring.lock().unwrap();
                    if r.len() >= STDERR_LINES {
                        r.pop_front();
                    }
                    r.push_back(line);
                }
            });
        }
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        self.kill = Some(kill_tx);
        let ev = self.ev_tx.clone();
        tokio::spawn(async move {
            let code = tokio::select! {
                s = child.wait() => s.ok().and_then(|s| s.code()),
                _ = kill_rx => {
                    let _ = child.start_kill();
                    child.wait().await.ok().and_then(|s| s.code())
                }
            };
            let _ = ev.send(Event::Exited(g, code));
        });
        // Ready once the port accepts connections.
        let ev = self.ev_tx.clone();
        let deadline = Instant::now() + self.spawn.opts.init_timeout;
        self.port_deadline = None;
        tokio::spawn(async move {
            while Instant::now() < deadline {
                if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                    let _ = ev.send(Event::PortReady(g));
                    return;
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            let _ = ev.send(Event::PortTimeout(g));
        });
        self.state = State::Starting;
        tracing::info!(upstream = self.spawn.id, name = %self.spawn.name, pid, port, "started multi-session server");
    }

    async fn stop_process(&mut self) {
        self.state = State::Stopping;
        let _ = self.url_tx.send(None);
        self.sessions.clear();
        if let Some(k) = self.kill.take() {
            let _ = k.send(());
        }
        if let Some(job) = self.job.take() {
            job.terminate();
        }
    }

    fn snapshot(&self) -> UpstreamStats {
        let mut sessions: Vec<SessionRef> = self.sessions.values().map(|s| s.info.clone()).collect();
        sessions.sort_by_key(|s| s.conn);
        UpstreamStats {
            id: self.spawn.id,
            name: self.spawn.name.clone(),
            share: Share::Native,
            scope: self.spawn.key.scope.clone(),
            command: self.spawn.spec.display(),
            pid: self.pid,
            state: self.state.label().to_string(),
            sessions,
            started_at_ms: self.started_at_ms,
            restarts: self.restarts,
            requests: self.requests,
            cache_hits: 0,
            last_error: self.last_error.clone(),
            stderr_tail: self.stderr.lock().unwrap().iter().rev().take(20).rev().cloned().collect(),
            launch: self.launch.as_ref().map(|l| l.display()),
            fast_path: self.launch.as_ref().is_some_and(|l| l.fast),
        }
    }
}

// ------------------------------------------------------------------ session

/// One agent session's MCP HTTP session.
struct SessionTask {
    http: reqwest::Client,
    url: watch::Receiver<Endpoint>,
    to_client: mpsc::UnboundedSender<String>,
    /// The session's own initialize request.
    init: Value,
    respond_init: bool,
    rx: mpsc::UnboundedReceiver<Value>,
}

#[derive(Clone)]
struct Ctx {
    http: reqwest::Client,
    url: String,
    sid: String,
    protocol: Option<String>,
    to_client: mpsc::UnboundedSender<String>,
}

impl SessionTask {
    async fn run(mut self) {
        let mut first = self.respond_init;
        let mut backlog: VecDeque<Value> = VecDeque::new();
        loop {
            // Wait for a running server, holding client messages meanwhile.
            let (generation, url) = loop {
                if let Some(ep) = self.url.borrow().clone() {
                    break ep;
                }
                tokio::select! {
                    changed = self.url.changed() => {
                        if changed.is_err() {
                            self.fail_all(&mut backlog, "server stopped");
                            return;
                        }
                    }
                    msg = self.rx.recv() => match msg {
                        Some(m) => backlog.push_back(m),
                        None => return,
                    }
                }
            };

            let ctx = match self.open(&url, first).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    if first {
                        let id = self.init.get("id").cloned().unwrap_or(Value::Null);
                        let _ = self.to_client.send(jsonrpc::to_line(&jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, format!("could not open a session: {e:#}"))));
                        return;
                    }
                    // Wait for the next server generation.
                    if self.url.changed().await.is_err() {
                        return;
                    }
                    continue;
                }
            };
            if !first {
                for n in ["notifications/tools/list_changed", "notifications/prompts/list_changed", "notifications/resources/list_changed"] {
                    let _ = self.to_client.send(jsonrpc::to_line(&jsonrpc::notification(n, None)));
                }
            }
            first = false;

            let listener = tokio::spawn(listen(ctx.clone()));
            let mut inflight = tokio::task::JoinSet::new();
            let restarted = loop {
                let next = if let Some(m) = backlog.pop_front() {
                    Some(m)
                } else {
                    tokio::select! {
                        m = self.rx.recv() => m,
                        changed = self.url.changed() => {
                            if changed.is_err() || self.url.borrow().as_ref().map(|e| e.0) != Some(generation) {
                                break true;
                            }
                            continue;
                        }
                        Some(_) = inflight.join_next(), if !inflight.is_empty() => continue,
                    }
                };
                match next {
                    Some(msg) => {
                        inflight.spawn(forward(ctx.clone(), msg));
                    }
                    None => break false,
                }
            };
            listener.abort();
            if !restarted {
                // The agent session ended: close the HTTP session politely.
                let _ = ctx.http.delete(&ctx.url).header("mcp-session-id", &ctx.sid).send().await;
                while inflight.join_next().await.is_some() {}
                return;
            }
            inflight.abort_all();
        }
    }

    fn fail_all(&self, backlog: &mut VecDeque<Value>, why: &str) {
        for msg in backlog.drain(..) {
            if let Kind::Request { id, .. } = jsonrpc::classify(&msg) {
                let _ = self.to_client.send(jsonrpc::to_line(&jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, why)));
            }
        }
    }

    /// Initializes a new HTTP session. Answers the client's initialize only the first time.
    async fn open(&self, url: &str, answer_client: bool) -> anyhow::Result<Ctx> {
        let resp = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(serde_json::to_vec(&self.init)?)
            .send()
            .await?;
        let status = resp.status();
        let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(String::from);
        let messages = read_messages(resp).await?;
        let init_id = self.init.get("id").cloned().unwrap_or(Value::Null);
        let answer = messages.into_iter().find(|m| m.get("id") == Some(&init_id) && (m.get("result").is_some() || m.get("error").is_some()));
        let Some(answer) = answer else { anyhow::bail!("no initialize answer (HTTP {status})") };
        if let Some(err) = answer.get("error") {
            if answer_client {
                let _ = self.to_client.send(jsonrpc::to_line(&answer));
            }
            anyhow::bail!("initialize failed: {err}");
        }
        let sid = sid.ok_or_else(|| anyhow::anyhow!("server did not assign a session id"))?;
        let protocol = answer.pointer("/result/protocolVersion").and_then(Value::as_str).map(String::from);
        if answer_client {
            let _ = self.to_client.send(jsonrpc::to_line(&answer));
        }
        let ctx = Ctx { http: self.http.clone(), url: url.to_string(), sid, protocol, to_client: self.to_client.clone() };
        forward(ctx.clone(), jsonrpc::notification("notifications/initialized", None)).await;
        Ok(ctx)
    }
}

fn request(ctx: &Ctx, method: reqwest::Method) -> reqwest::RequestBuilder {
    let mut b = ctx.http.request(method, &ctx.url).header("mcp-session-id", &ctx.sid).header("accept", "application/json, text/event-stream");
    if let Some(p) = &ctx.protocol {
        b = b.header("mcp-protocol-version", p);
    }
    b
}

/// POSTs one client message and relays whatever the server answers.
async fn forward(ctx: Ctx, msg: Value) {
    let id = match jsonrpc::classify(&msg) {
        Kind::Request { id, .. } => Some(id.clone()),
        _ => None,
    };
    let body = match serde_json::to_vec(&msg) {
        Ok(b) => b,
        Err(_) => return,
    };
    let fail = |why: String| {
        if let Some(id) = &id {
            let _ = ctx.to_client.send(jsonrpc::to_line(&jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, why)));
        }
    };
    let resp = match request(&ctx, reqwest::Method::POST).header("content-type", "application/json").body(body).send().await {
        Ok(r) => r,
        Err(e) => return fail(format!("server unreachable: {e}")),
    };
    let status = resp.status();
    if status == reqwest::StatusCode::ACCEPTED || status == reqwest::StatusCode::NO_CONTENT {
        return;
    }
    if !status.is_success() {
        return fail(format!("server answered HTTP {status}"));
    }
    if is_sse(&resp) {
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            for data in parser.feed(&chunk) {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    let _ = ctx.to_client.send(jsonrpc::to_line(&v));
                }
            }
        }
    } else {
        match read_messages(resp).await {
            Ok(msgs) => {
                for m in msgs {
                    let _ = ctx.to_client.send(jsonrpc::to_line(&m));
                }
            }
            Err(e) => fail(format!("bad answer from server: {e}")),
        }
    }
}

/// The standalone GET stream for messages the server starts on its own.
async fn listen(ctx: Ctx) {
    let Ok(resp) = request(&ctx, reqwest::Method::GET).send().await else { return };
    if !resp.status().is_success() || !is_sse(&resp) {
        return; // 405: the server does not offer one
    }
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::default();
    while let Some(Ok(chunk)) = stream.next().await {
        for data in parser.feed(&chunk) {
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                let _ = ctx.to_client.send(jsonrpc::to_line(&v));
            }
        }
    }
}

fn is_sse(resp: &reqwest::Response) -> bool {
    resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("text/event-stream"))
}

async fn read_messages(resp: reqwest::Response) -> anyhow::Result<Vec<Value>> {
    let sse = is_sse(&resp);
    let bytes = resp.bytes().await?;
    if sse {
        let mut parser = SseParser::default();
        let mut out: Vec<Value> = parser.feed(&bytes).into_iter().filter_map(|d| serde_json::from_str(&d).ok()).collect();
        out.extend(parser.finish().and_then(|d| serde_json::from_str(&d).ok()));
        return Ok(out);
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    Ok(match serde_json::from_slice::<Value>(&bytes)? {
        Value::Array(items) => items,
        v => vec![v],
    })
}

/// Incremental parser for `text/event-stream`. Yields each event's data.
#[derive(Default)]
pub struct SseParser {
    line: Vec<u8>,
    data: String,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.on_line(&String::from_utf8_lossy(&line), &mut events);
            } else {
                self.line.push(b);
            }
        }
        events
    }

    fn on_line(&mut self, line: &str, events: &mut Vec<String>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                events.push(std::mem::take(&mut self.data));
            }
            return;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // `event:`, `id:`, `retry:` and `:` comments carry nothing we need.
    }

    /// Flushes an event that was not followed by a blank line.
    pub fn finish(&mut self) -> Option<String> {
        if !self.line.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
            let mut ev = Vec::new();
            self.on_line(line.trim_end_matches('\r'), &mut ev);
        }
        (!self.data.is_empty()).then(|| std::mem::take(&mut self.data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_handles_split_chunks_and_multiline_data() {
        let mut p = SseParser::default();
        let mut out = p.feed(b"event: message\r\ndata: {\"a\"");
        assert!(out.is_empty());
        out.extend(p.feed(b":1}\r\n\r\n: comment\n\ndata: line1\ndata: line2\n\n"));
        assert_eq!(out, vec!["{\"a\":1}".to_string(), "line1\nline2".to_string()]);
        p.feed(b"data: tail");
        assert_eq!(p.finish().as_deref(), Some("tail"));
    }

    #[test]
    fn playwright_needs_isolated_for_native_mode() {
        let s = |args: &[&str]| ServerSpec { command: "npx".into(), args: args.iter().map(|a| a.to_string()).collect(), env: Default::default() };
        assert!(native_args(Some("@playwright/mcp"), &s(&["@playwright/mcp", "--isolated", "--headless"])).is_some());
        assert!(native_args(Some("@playwright/mcp"), &s(&["@playwright/mcp", "--headless"])).is_none(), "persistent profile cannot be shared");
        assert!(native_args(Some("@playwright/mcp"), &s(&["@playwright/mcp", "--isolated", "--port", "9"])).is_none());
        assert!(native_args(Some("other"), &s(&["--isolated"])).is_none());
    }
}
