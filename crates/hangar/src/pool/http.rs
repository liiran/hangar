//! Servers that natively serve many clients over Streamable HTTP.
//!
//! Some servers keep per-client state but can also run in HTTP mode, where
//! each HTTP session gets its own state inside one process. Playwright MCP
//! with `--isolated` is the important one: each session gets its own browser
//! context (own pages, cookies and storage) in one shared browser.
//!
//! Hangar starts one such process and opens one MCP HTTP session per agent
//! session. Ids, progress tokens and server-to-client requests are per HTTP
//! session already, so messages pass through unchanged. Every request gets
//! an answer: if the server dies, restarts or ends a stream without
//! answering, the session receives an error instead of waiting forever.

use super::process::{self, Ring};
use super::upstream::{Cmd, FAILED_RETRY, Rejected, SessionRef, Upstream, UpstreamStats, display, launch_display};
use super::{PoolKey, PoolOptions};
use crate::jsonrpc::{self, Kind};
use crate::launch::{self, FastPath, Launch};
use crate::policy::Share;
use crate::util::Locked;
use crate::util::now_ms;
use futures_util::StreamExt;
use hangar_proto::ServerSpec;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

const MAX_CRASHES: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// How often a start that ends before the port answers is tried again on another port.
const START_TRIES: u8 = 2;
/// How long a port must keep answering, with the server still running, before it counts as ready.
const SETTLE: Duration = Duration::from_millis(150);

/// Extra arguments that switch a known server into multi-client HTTP mode.
/// `{port}` is replaced with a free loopback port.
pub fn native_args(package: Option<&str>, spec: &ServerSpec) -> Option<Vec<String>> {
    let has = |flag: &str| spec.args.iter().any(|a| a == flag || a.starts_with(&format!("{flag}=")));
    match package? {
        "@playwright/mcp" => {
            let conflicting = ["--port", "--host", "--extension", "--cdp-endpoint", "--endpoint", "--shared-browser-context"];
            (has("--isolated") && !conflicting.iter().any(|f| has(f))).then(|| vec!["--port".into(), "{port}".into(), "--host".into(), "127.0.0.1".into()])
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
    pub on_exit: Weak<super::OnExit>,
    /// The pool's running total for this server name.
    pub served: Arc<std::sync::atomic::AtomicU64>,
}

/// Where sessions can reach the server.
#[derive(Clone, Debug, PartialEq)]
enum Endpoint {
    /// Starting or restarting.
    Waiting,
    /// (process generation, base URL)
    Ready(u64, String),
    /// Not running; requests fail with this reason until a retry.
    Failed(String),
    /// Shut down for good.
    Gone,
}

pub fn start(spawn: HttpSpawn) -> Upstream {
    let (tx, rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (url_tx, _) = watch::channel(Endpoint::Waiting);
    let handle = Upstream::from_parts(spawn.id, spawn.key.clone(), tx);
    let http =
        reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(5)).build().expect("http client");
    let actor = HttpActor {
        s: spawn,
        http,
        rx,
        ev_tx,
        ev_rx,
        url_tx,
        sessions: HashMap::new(),
        generation: 0,
        state: State::Starting,
        proc_: None,
        port: 0,
        crashes: VecDeque::new(),
        retry_at: None,
        idle_since: None,
        started_at_ms: None,
        restarts: 0,
        start_tries: 0,
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
    s: HttpSpawn,
    http: reqwest::Client,
    rx: mpsc::UnboundedReceiver<Cmd>,
    ev_tx: mpsc::UnboundedSender<Event>,
    ev_rx: mpsc::UnboundedReceiver<Event>,
    url_tx: watch::Sender<Endpoint>,
    sessions: HashMap<u64, SessionHandle>,
    generation: u64,
    state: State,
    /// The running process; its kill switch ends the waiter task.
    proc_: Option<(u32, Option<crate::procs::Job>, oneshot::Sender<()>)>,
    port: u16,
    crashes: VecDeque<Instant>,
    /// When to restart: after a crash (backoff) or a failure (retry).
    retry_at: Option<Instant>,
    idle_since: Option<Instant>,
    started_at_ms: Option<u64>,
    restarts: u64,
    /// Starts that ended before the port answered, since the last good one.
    start_tries: u8,
    requests: u64,
    last_error: Option<String>,
    stderr: Ring,
    launch: Option<Launch>,
    fast_failed: bool,
    done: bool,
}

// Lives for one loop iteration on the stack; boxing would only add an allocation.
#[allow(clippy::large_enum_variant)]
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
                    self.stop_process();
                    self.done = true;
                }
                Wake::Cmd(Some(cmd)) => self.on_cmd(cmd),
                Wake::Ev(Some(ev)) => self.on_event(ev),
                Wake::Ev(None) => {}
                Wake::Timer => self.on_timer(),
            }
        }
        let _ = self.url_tx.send(Endpoint::Gone);
        if let Some(cb) = self.s.on_exit.upgrade() {
            cb(self.s.id, &self.s.key);
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

    fn idle_limit(&self) -> Duration {
        self.s.opts.idle_timeout
    }

    fn next_deadline(&self) -> Option<Instant> {
        [self.retry_at.filter(|_| !self.sessions.is_empty()), self.idle_since.map(|s| s + self.idle_limit())].into_iter().flatten().min()
    }

    fn on_timer(&mut self) {
        let now = Instant::now();
        if self.retry_at.is_some_and(|t| now >= t) && !self.sessions.is_empty() {
            self.retry_at = None;
            self.spawn_process();
        }
        if self.sessions.is_empty() && self.idle_since.is_some_and(|s| now >= s + self.idle_limit()) {
            tracing::info!(upstream = self.s.id, name = %self.s.name, "stopping idle server");
            self.stop_process();
            self.done = true;
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) {
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
                self.sessions.insert(info.conn, SessionHandle { info: info.summary(), tx });
                if self.state == State::Failed && self.retry_at.is_some_and(|t| Instant::now() >= t) {
                    self.retry_at = None;
                    self.spawn_process();
                }
                let _ = reply.send(Ok(()));
            }
            Cmd::Client { conn, msg } => {
                if let Some(s) = self.sessions.get(&conn) {
                    if matches!(jsonrpc::classify(&msg), Kind::Request { .. }) {
                        self.requests += 1;
                        self.s.served.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                self.stop_process();
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
                self.state = State::Ready;
                self.start_tries = 0;
                if self.launch.as_ref().is_some_and(|l| !l.fast) {
                    launch::note_npx_ran(self.s.opts.state_dir.as_deref(), &self.s.spec);
                }
                let url = format!("http://127.0.0.1:{}/mcp", self.port);
                tracing::info!(upstream = self.s.id, name = %self.s.name, %url, "multi-session server ready");
                let _ = self.url_tx.send(Endpoint::Ready(g, url));
            }
            Event::PortTimeout(g) if g == self.generation => self.fail("did not start listening in time".into()),
            Event::Exited(g, code) if g == self.generation => {
                self.proc_ = None;
                self.generation += 1;
                let reason = code.map(|c| format!("server exited with code {c}")).unwrap_or_else(|| "server exited".into());
                if self.state == State::Starting {
                    // The port was free when it was picked, but it is not held until the
                    // server takes it. If another program got there first, the server
                    // exits at once: try again on another port before giving up.
                    if self.start_tries < START_TRIES {
                        self.start_tries += 1;
                        tracing::warn!(upstream = self.s.id, name = %self.s.name, "{reason} while starting; trying another port");
                        self.spawn_process();
                        return;
                    }
                    self.fail(reason);
                    return;
                }
                tracing::warn!(upstream = self.s.id, name = %self.s.name, "{reason}");
                self.last_error = Some(reason);
                let _ = self.url_tx.send(Endpoint::Waiting);
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
                    self.fail(format!("crashed {} times in a minute", self.crashes.len()));
                    return;
                }
                self.state = State::Restarting;
                self.retry_at = Some(now + Duration::from_millis(250 * (1 << (self.crashes.len() - 1).min(5))));
            }
            _ => {}
        }
    }

    fn fail(&mut self, reason: String) {
        self.kill();
        if self.launch.as_ref().is_some_and(|l| l.fast) && !self.fast_failed {
            tracing::warn!(upstream = self.s.id, "fast start failed ({reason}); using the configured command");
            self.fast_failed = true;
            self.spawn_process();
            return;
        }
        let tail = process::tail(&self.stderr, 1).pop().filter(|l| !l.trim().is_empty());
        let reason = tail.map_or(reason.clone(), |l| format!("{reason} ({})", l.chars().take(200).collect::<String>()));
        tracing::warn!(upstream = self.s.id, name = %self.s.name, "{reason}");
        self.last_error = Some(reason.clone());
        self.state = State::Failed;
        self.retry_at = Some(Instant::now() + FAILED_RETRY);
        let _ = self.url_tx.send(Endpoint::Failed(format!("{} is unavailable: {reason}", self.s.name)));
        if self.sessions.is_empty() {
            self.idle_since = Some(Instant::now());
        }
    }

    fn spawn_process(&mut self) {
        self.generation += 1;
        let g = self.generation;
        let _ = self.url_tx.send(Endpoint::Waiting);
        let port = match std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()) {
            Ok(a) => a.port(),
            Err(e) => return self.fail(format!("no free port: {e}")),
        };
        self.port = port;
        let fast = FastPath { enabled: self.s.opts.fast_npx && !self.fast_failed, state_dir: self.s.opts.state_dir.as_deref() };
        let launch = launch::plan(&self.s.spec, &self.s.env, fast);
        let extra: Vec<String> = self.s.extra_args.iter().map(|a| a.replace("{port}", &port.to_string())).collect();
        let opts = process::Options {
            launch: &launch,
            extra_args: &extra,
            cwd: &self.s.cwd,
            env: &self.s.env,
            declared: &self.s.spec.env,
            stdin: false,
            stdout: false,
        };
        let started = process::start(opts, &self.stderr);
        self.launch = Some(launch);
        let mut started = match started {
            Ok(s) => s,
            Err(e) => {
                let program = self.launch.as_ref().map(|l| l.program.display().to_string()).unwrap_or_default();
                return self.fail(format!("could not start {program}: {e}"));
            }
        };
        let pid = started.pid;
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let job = started.job.take();
        let ev = self.ev_tx.clone();
        let mut child = started.child;
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
        self.proc_ = Some((pid, job, kill_tx));
        self.started_at_ms = Some(now_ms());
        // Ready once the port accepts connections.
        let ev = self.ev_tx.clone();
        let deadline = Instant::now() + self.s.opts.init_timeout;
        tokio::spawn(async move {
            while Instant::now() < deadline {
                if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                    // Something accepts on the port. If it is not the server (another program
                    // took the port first), the server fails to take it and exits within
                    // moments, and that exit ends this start. So wait a moment and look again
                    // before calling it ready.
                    tokio::time::sleep(SETTLE).await;
                    if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                        let _ = ev.send(Event::PortReady(g));
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            let _ = ev.send(Event::PortTimeout(g));
        });
        self.state = State::Starting;
        tracing::info!(upstream = self.s.id, name = %self.s.name, pid, port, "started multi-session server");
    }

    /// Kills the current process tree; later events from it are ignored.
    fn kill(&mut self) {
        self.generation += 1;
        if let Some((_, job, kill)) = self.proc_.take() {
            let _ = kill.send(());
            if let Some(job) = job {
                job.terminate();
            }
        }
    }

    fn stop_process(&mut self) {
        self.state = State::Stopping;
        let _ = self.url_tx.send(Endpoint::Gone);
        self.sessions.clear();
        self.kill();
    }

    fn snapshot(&self) -> UpstreamStats {
        let mut sessions: Vec<SessionRef> = self.sessions.values().map(|s| s.info.clone()).collect();
        sessions.sort_by_key(|s| s.conn);
        UpstreamStats {
            id: self.s.id,
            name: self.s.name.clone(),
            share: Share::Native,
            scope: self.s.key.scope.clone(),
            command: display(&self.s.spec.command, &self.s.spec.args),
            pid: self.proc_.as_ref().map(|p| p.0),
            state: self.state.label().to_string(),
            sessions,
            started_at_ms: self.started_at_ms,
            restarts: self.restarts,
            requests: self.requests,
            cache_hits: 0,
            last_error: self.last_error.clone(),
            stderr_tail: process::tail(&self.stderr, 20),
            launch: self.launch.as_ref().map(launch_display),
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

/// Requests sent to the server and not yet answered: id key -> id.
type InFlight = Arc<Mutex<HashMap<String, Value>>>;

#[derive(Clone)]
struct Ctx {
    http: reqwest::Client,
    url: String,
    sid: String,
    protocol: Option<String>,
    to_client: mpsc::UnboundedSender<String>,
    inflight: InFlight,
}

impl Ctx {
    fn reply_error(&self, id: &Value, why: &str) {
        let _ = self.to_client.send(jsonrpc::to_line(&jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, why)));
    }

    /// Relays a server message; marks its request answered if it is a response.
    fn relay(&self, msg: &Value) {
        if let Kind::Response { id } = jsonrpc::classify(msg) {
            self.inflight.locked().remove(&jsonrpc::id_key(id));
        }
        let _ = self.to_client.send(jsonrpc::to_line(msg));
    }

    /// Answers every unanswered request with an error.
    fn fail_inflight(&self, why: &str) {
        let ids: Vec<Value> = self.inflight.locked().drain().map(|(_, v)| v).collect();
        for id in ids {
            self.reply_error(&id, why);
        }
    }
}

impl SessionTask {
    async fn run(mut self) {
        let mut first = self.respond_init;
        let mut backlog: VecDeque<Value> = VecDeque::new();
        loop {
            // Wait for a running server, holding client messages meanwhile.
            let (generation, url) = loop {
                let ep = self.url.borrow().clone();
                match ep {
                    Endpoint::Ready(g, u) => break (g, u),
                    Endpoint::Gone => {
                        self.fail_backlog(&mut backlog, "the server was stopped");
                        return;
                    }
                    Endpoint::Failed(why) => {
                        if first {
                            first = false;
                            let id = self.init.get("id").cloned().unwrap_or(Value::Null);
                            self.send_error(&id, &why);
                        }
                        self.fail_backlog(&mut backlog, &why);
                    }
                    Endpoint::Waiting => {}
                }
                tokio::select! {
                    changed = self.url.changed() => {
                        if changed.is_err() {
                            self.fail_backlog(&mut backlog, "the server was stopped");
                            return;
                        }
                    }
                    msg = self.rx.recv() => match msg {
                        Some(m) => {
                            let failed = match &*self.url.borrow() { Endpoint::Failed(w) => Some(w.clone()), _ => None };
                            match failed {
                                Some(why) => {
                                    if let Kind::Request { id, .. } = jsonrpc::classify(&m) {
                                        self.send_error(id, &why);
                                    }
                                }
                                None => backlog.push_back(m),
                            }
                        }
                        None => return,
                    }
                }
            };

            let ctx = match self.open(&url, first).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    if first {
                        // The client's initialize was answered with the error in `open`.
                        tracing::debug!("could not open an HTTP session: {e:#}");
                        return;
                    }
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
                            let same = matches!(&*self.url.borrow(), Endpoint::Ready(g, _) if *g == generation);
                            if changed.is_err() || !same {
                                break true;
                            }
                            continue;
                        }
                        Some(_) = inflight.join_next(), if !inflight.is_empty() => continue,
                    }
                };
                match next {
                    // hangar already sent `initialized` for this HTTP session.
                    Some(msg) if msg.get("method").and_then(Value::as_str) == Some("notifications/initialized") => {}
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
                inflight.abort_all();
                return;
            }
            inflight.abort_all();
            ctx.fail_inflight("the server restarted while this request was running; try again");
        }
    }

    fn send_error(&self, id: &Value, why: &str) {
        let _ = self.to_client.send(jsonrpc::to_line(&jsonrpc::error(id.clone(), jsonrpc::SERVER_UNAVAILABLE, why)));
    }

    fn fail_backlog(&self, backlog: &mut VecDeque<Value>, why: &str) {
        for msg in backlog.drain(..) {
            if let Kind::Request { id, .. } = jsonrpc::classify(&msg) {
                self.send_error(id, why);
            }
        }
    }

    /// Initializes a new HTTP session. Answers the client's initialize only the first time.
    async fn open(&self, url: &str, answer_client: bool) -> anyhow::Result<Ctx> {
        let init_id = self.init.get("id").cloned().unwrap_or(Value::Null);
        let outcome = async {
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
            let answer = messages.into_iter().find(|m| m.get("id") == Some(&init_id) && (m.get("result").is_some() || m.get("error").is_some()));
            let Some(answer) = answer else { anyhow::bail!("no initialize answer (HTTP {status})") };
            anyhow::Ok((sid, answer))
        }
        .await;
        let (sid, answer) = match outcome {
            Ok(v) => v,
            Err(e) => {
                if answer_client {
                    self.send_error(&init_id, &format!("could not open a session: {e:#}"));
                }
                return Err(e);
            }
        };
        if let Some(err) = answer.get("error") {
            if answer_client {
                let _ = self.to_client.send(jsonrpc::to_line(&answer));
            }
            anyhow::bail!("initialize failed: {err}");
        }
        let Some(sid) = sid else {
            if answer_client {
                self.send_error(&init_id, "the server did not assign a session id");
            }
            anyhow::bail!("server did not assign a session id");
        };
        let protocol = answer.pointer("/result/protocolVersion").and_then(Value::as_str).map(String::from);
        if answer_client {
            let _ = self.to_client.send(jsonrpc::to_line(&answer));
        }
        let ctx = Ctx {
            http: self.http.clone(),
            url: url.to_string(),
            sid,
            protocol,
            to_client: self.to_client.clone(),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        };
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

/// POSTs one client message and relays whatever the server answers. A
/// request always ends with exactly one answer to the client.
async fn forward(ctx: Ctx, msg: Value) {
    let id = match jsonrpc::classify(&msg) {
        Kind::Request { id, .. } => Some(id.clone()),
        _ => None,
    };
    let key = id.as_ref().map(jsonrpc::id_key);
    if let (Some(id), Some(key)) = (&id, &key) {
        ctx.inflight.locked().insert(key.clone(), id.clone());
    }
    let answered = |ctx: &Ctx| key.as_ref().is_none_or(|k| !ctx.inflight.locked().contains_key(k));
    let fail = |ctx: &Ctx, why: String| {
        if let (Some(id), Some(key)) = (&id, &key)
            && ctx.inflight.locked().remove(key).is_some()
        {
            ctx.reply_error(id, &why);
        }
    };
    let Ok(body) = serde_json::to_vec(&msg) else { return };
    let resp = match request(&ctx, reqwest::Method::POST).header("content-type", "application/json").body(body).send().await {
        Ok(r) => r,
        Err(e) => return fail(&ctx, format!("server unreachable: {e}")),
    };
    let status = resp.status();
    if status == reqwest::StatusCode::ACCEPTED || status == reqwest::StatusCode::NO_CONTENT {
        return fail(&ctx, "the server accepted the request without answering it".into());
    }
    if !status.is_success() {
        return fail(&ctx, format!("server answered HTTP {status}"));
    }
    if is_sse(&resp) {
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            for data in parser.feed(&chunk) {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    ctx.relay(&v);
                }
            }
        }
        if let Some(v) = parser.finish().and_then(|d| serde_json::from_str::<Value>(&d).ok()) {
            ctx.relay(&v);
        }
    } else {
        match read_messages(resp).await {
            Ok(msgs) => msgs.iter().for_each(|m| ctx.relay(m)),
            Err(e) => return fail(&ctx, format!("bad answer from server: {e}")),
        }
    }
    if !answered(&ctx) {
        fail(&ctx, "the server ended the response without answering".into());
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
                ctx.relay(&v);
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
