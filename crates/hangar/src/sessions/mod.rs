//! Agent sessions: start, watch, answer and pause many Claude Code sessions.
//!
//! Each session runs `claude` headless (see [`claude`]). Its events are kept
//! on disk ([`store`]) and pushed to the UI. The [`Budget`] decides when a
//! session may start; sessions that cannot start yet wait in a queue, and
//! idle sessions can be paused (process stopped, resumed with `--resume` on
//! the next message) to make room. A message the user sends always reaches
//! Claude eventually: it waits in the session's outbox (saved to disk) until
//! a running process has confirmed it started.

pub mod claude;
mod store;
pub mod trust;

use crate::budget::{Budget, Machine};
use crate::config::Config;
use crate::procs;
use crate::util::now_ms;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::Store;
use tokio::sync::{broadcast, mpsc};

/// Longest text kept from one tool result; the full output stays in Claude's
/// own transcript.
const MAX_BLOCK_TEXT: usize = 16 * 1024;
/// Events kept in memory per session (older ones stay on disk).
const MAX_EVENTS_IN_MEMORY: usize = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Waiting for the memory budget.
    Queued,
    Starting,
    Working,
    /// Waiting for a permission decision.
    NeedsYou,
    /// Finished its turn; waiting for the next message.
    Idle,
    /// Process stopped to save memory; the next message resumes it.
    Hibernated,
    Stopped,
    Error,
}

impl Status {
    pub fn is_live(self) -> bool {
        matches!(self, Status::Starting | Status::Working | Status::NeedsYou | Status::Idle)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    #[serde(default)]
    pub model: Option<String>,
    pub permission_mode: String,
    /// Claude's own session id, used to resume.
    #[serde(default)]
    pub claude_session_id: Option<String>,
    pub status: Status,
    #[serde(default)]
    pub status_detail: Option<String>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub preview: String,
    /// Peak memory of the session's process tree.
    #[serde(default)]
    pub peak_bytes: u64,
    /// Messages Claude has not received yet (queued, paused, restarting).
    #[serde(default)]
    pub outbox: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ts: u64,
    #[serde(flatten)]
    pub body: EventBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventBody {
    /// A message the user sent.
    User {
        text: String,
    },
    /// A stream-json message from Claude Code (assistant, user/tool results,
    /// result, system). Results carry `turn_cost_usd`, the cost of that turn.
    Claude {
        msg: Value,
    },
    /// Claude asks for permission to use a tool.
    Permission {
        request_id: String,
        tool_name: String,
        input: Value,
        #[serde(default)]
        suggestions: Option<Value>,
        #[serde(default)]
        description: Option<String>,
    },
    /// `allow`, `allow_always` (rules remembered for this session), `deny` or `cancelled`.
    Decision {
        request_id: String,
        behavior: String,
    },
    Status {
        status: Status,
        #[serde(default)]
        detail: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingPermission {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub suggestions: Option<Value>,
    /// Whether "always allow" has session rules to remember.
    pub can_remember: bool,
}

/// What the UI sees for one session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    #[serde(flatten)]
    pub meta: SessionMeta,
    /// Increases with every view built. Views are built under the manager's
    /// lock, so a higher `rev` is always newer state: the UI uses it to drop
    /// updates that arrive after a fresher snapshot.
    pub rev: u64,
    pub pid: Option<u32>,
    pub memory_bytes: Option<u64>,
    pub pending: Vec<PendingPermission>,
    pub queue_position: Option<usize>,
    pub mcp: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    Session { session: SessionView },
    Event { session_id: String, event: Event },
    Removed { session_id: String },
}

pub struct NewSession {
    pub cwd: String,
    pub prompt: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    /// The user confirmed they trust the folder.
    pub trust: bool,
}

/// `create` refused because the folder is not trusted yet.
#[derive(Debug)]
pub struct NeedsTrust(pub trust::TrustInfo);

impl std::fmt::Display for NeedsTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this folder is not trusted yet")
    }
}

impl std::error::Error for NeedsTrust {}

pub const PERMISSION_MODES: &[&str] = &["default", "acceptEdits", "plan", "bypassPermissions"];

/// The running Claude process of a session.
struct Run {
    driver: claude::Driver,
    /// Claude reported `system/init`: it is up and reading messages.
    initialized: bool,
    /// Messages written before `initialized`; re-sent if the process dies first.
    unconfirmed: Vec<String>,
    /// Last cumulative `total_cost_usd` reported by this process.
    cost_seen: f64,
    completed_turns: u64,
    resumed: bool,
}

struct Session {
    meta: SessionMeta,
    events: Vec<Event>,
    next_seq: u64,
    run: Option<Run>,
    pending: BTreeMap<String, PendingPermission>,
    idle_since: Option<Instant>,
    memory: Option<u64>,
    mcp: Vec<Value>,
    /// Set when we stop the process on purpose: the status to land in.
    stopping: Option<Status>,
}

impl Session {
    fn new(meta: SessionMeta, next_seq: u64) -> Session {
        Session { meta, events: Vec::new(), next_seq, run: None, pending: BTreeMap::new(), idle_since: None, memory: None, mcp: Vec::new(), stopping: None }
    }

    fn pid(&self) -> Option<u32> {
        self.run.as_ref().map(|r| r.driver.pid)
    }
}

pub struct Manager {
    home: PathBuf,
    store: Store,
    claude_bin: Mutex<Option<PathBuf>>,
    claude_json: Mutex<PathBuf>,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<UiEvent>,
    driver_tx: mpsc::UnboundedSender<claude::DriverEvent>,
    hibernate_after: Option<Duration>,
    reclaim_after: Duration,
    /// Set by `shutdown`: start nothing new.
    closing: std::sync::atomic::AtomicBool,
}

struct Inner {
    sessions: HashMap<String, Session>,
    queue: Vec<String>,
    budget: Budget,
}

impl Manager {
    pub fn start(home: &Path, config: &Config) -> Arc<Manager> {
        let (tx, _) = broadcast::channel(4096);
        let (driver_tx, driver_rx) = mpsc::unbounded_channel();
        let b = &config.budget;
        let manager = Arc::new(Manager {
            home: home.to_path_buf(),
            store: Store::new(home),
            claude_bin: Mutex::new(None),
            claude_json: Mutex::new(crate::claude_config::claude_json_path()),
            inner: Mutex::new(Inner { sessions: HashMap::new(), queue: Vec::new(), budget: Budget::new(b) }),
            tx,
            driver_tx,
            hibernate_after: (b.hibernate_after_mins > 0).then(|| Duration::from_secs(b.hibernate_after_mins * 60)),
            reclaim_after: Duration::from_secs(b.reclaim_idle_after_secs),
            closing: std::sync::atomic::AtomicBool::new(false),
        });
        manager.load();
        tokio::spawn(manager.clone().driver_loop(driver_rx));
        tokio::spawn(manager.clone().tick_loop());
        manager
    }

    pub fn set_claude_bin(&self, path: Option<PathBuf>) {
        *self.claude_bin.lock().unwrap() = path;
    }

    /// Where Claude Code's own trust decisions are read from (tests point it elsewhere).
    pub fn set_claude_json(&self, path: PathBuf) {
        *self.claude_json.lock().unwrap() = path;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UiEvent> {
        self.tx.subscribe()
    }

    fn load(&self) {
        let mut inner = self.inner.lock().unwrap();
        let mut waiting = Vec::new();
        for (mut meta, next_seq) in self.store.load_all() {
            // The processes are gone after a daemon restart.
            if meta.status.is_live() {
                meta.status = if meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped };
                meta.status_detail = None;
            }
            // Messages the user already sent still have to reach Claude.
            if meta.status == Status::Queued || !meta.outbox.is_empty() && meta.status != Status::Error {
                meta.status = Status::Queued;
                waiting.push((meta.created_ms, meta.id.clone()));
            }
            inner.sessions.insert(meta.id.clone(), Session::new(meta, next_seq));
        }
        waiting.sort();
        inner.queue = waiting.into_iter().map(|(_, id)| id).collect();
    }

    // ------------------------------------------------------------ queries

    pub fn list(&self) -> Vec<SessionView> {
        let inner = self.inner.lock().unwrap();
        let mut v: Vec<SessionView> = inner.sessions.values().map(|s| view(s, &inner.queue)).collect();
        v.sort_by_key(|v| std::cmp::Reverse(v.meta.created_ms));
        v
    }

    pub fn machine(&self) -> Machine {
        self.inner.lock().unwrap().budget.machine()
    }

    /// Events after `after`, oldest first.
    pub fn events(&self, id: &str, after: u64) -> Result<Vec<Event>> {
        {
            let inner = self.inner.lock().unwrap();
            let s = inner.sessions.get(id).context("no such session")?;
            if s.events.first().is_some_and(|first| first.seq <= after + 1) {
                return Ok(s.events.iter().filter(|e| e.seq > after).cloned().collect());
            }
        }
        Ok(self.store.events(id, after))
    }

    pub fn recent_projects(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut seen: Vec<(u64, String)> = inner.sessions.values().map(|s| (s.meta.updated_ms, s.meta.cwd.clone())).collect();
        seen.sort_by_key(|s| std::cmp::Reverse(s.0));
        let mut out: Vec<String> = Vec::new();
        for (_, cwd) in seen {
            if !out.iter().any(|c| c.eq_ignore_ascii_case(&cwd)) {
                out.push(cwd);
            }
        }
        out
    }

    /// Whether `cwd` is trusted, and what it would run.
    pub fn trust(&self, cwd: &Path) -> trust::TrustInfo {
        trust::inspect(&self.home, &self.claude_json.lock().unwrap(), cwd)
    }

    // ------------------------------------------------------------ commands

    pub fn create(self: &Arc<Self>, req: NewSession) -> Result<SessionView> {
        let cwd = PathBuf::from(req.cwd.trim());
        if !cwd.is_dir() {
            bail!("{} is not a folder", cwd.display());
        }
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("the first message is empty");
        }
        let mode = req.permission_mode.unwrap_or_else(|| "default".into());
        if !PERMISSION_MODES.contains(&mode.as_str()) {
            bail!("unknown permission mode {mode}");
        }
        let info = self.trust(&cwd);
        if !info.trusted {
            if !req.trust {
                bail!(NeedsTrust(info));
            }
            trust::remember(&self.home, &cwd);
        }
        let id = new_id();
        let now = now_ms();
        let title = req.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| title_from(&prompt));
        let meta = SessionMeta {
            id: id.clone(),
            title,
            cwd: cwd.display().to_string(),
            created_ms: now,
            updated_ms: now,
            model: req.model.filter(|m| !m.is_empty()),
            permission_mode: mode,
            claude_session_id: None,
            status: Status::Queued,
            status_detail: None,
            cost_usd: 0.0,
            turns: 0,
            preview: String::new(),
            peak_bytes: 0,
            outbox: vec![prompt.clone()],
        };
        self.store.create(&id)?;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.sessions.insert(id.clone(), Session::new(meta, 1));
            self.push_event(&mut inner, &id, EventBody::User { text: prompt });
            inner.queue.push(id.clone());
            self.save(&inner, &id);
        }
        self.pump_queue();
        self.view_of(&id).context("session vanished")
    }

    /// Sends a follow-up message, resuming the session if it is not running.
    pub fn send(self: &Arc<Self>, id: &str, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("empty message");
        }
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let deliverable = s.stopping.is_none() && s.meta.status.is_live();
        match s.run.as_mut().filter(|_| deliverable) {
            Some(run) => {
                run.driver.send_user(text);
                if !run.initialized {
                    run.unconfirmed.push(text.to_string());
                }
                s.idle_since = None;
                if s.meta.status == Status::Idle {
                    s.meta.status = Status::Working;
                    s.meta.status_detail = None;
                }
            }
            None => {
                // Not running, or on its way out: keep the message and
                // (re)start the session once the old process is gone.
                s.meta.outbox.push(text.to_string());
                let exiting = s.run.is_some();
                if !exiting {
                    s.meta.status = Status::Queued;
                    s.meta.status_detail = None;
                }
                if !inner.queue.iter().any(|q| q == id) {
                    inner.queue.push(id.to_string());
                }
            }
        }
        self.push_event(&mut inner, id, EventBody::User { text: text.to_string() });
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        drop(inner);
        self.pump_queue();
        Ok(())
    }

    pub fn decide(&self, id: &str, request_id: &str, allow: bool, always: bool, message: Option<String>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let run = s.run.as_ref().context("session is not running")?;
        let p = s.pending.remove(request_id).context("that request is no longer pending")?;
        let behavior = if allow {
            let remembered = run.driver.allow(request_id, &p.input, if always { p.suggestions.as_ref() } else { None });
            if remembered { "allow_always" } else { "allow" }
        } else {
            run.driver.deny(request_id, message.as_deref().unwrap_or("The user declined this tool use."));
            "deny"
        };
        let still_waiting = !s.pending.is_empty();
        self.push_event(&mut inner, id, EventBody::Decision { request_id: request_id.into(), behavior: behavior.into() });
        if !still_waiting {
            set_status(&mut inner, id, Status::Working, None);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        Ok(())
    }

    pub fn interrupt(&self, id: &str) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        let s = inner.sessions.get(id).context("no such session")?;
        s.run.as_ref().context("session is not running")?.driver.interrupt();
        Ok(())
    }

    /// Stops the process and drops undelivered messages. The conversation can be resumed later.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.retain(|q| q != id);
        let s = inner.sessions.get_mut(id).context("no such session")?;
        s.meta.outbox.clear();
        if let Some(run) = &s.run {
            s.stopping = Some(Status::Stopped);
            run.driver.kill();
        } else {
            set_status(&mut inner, id, Status::Stopped, None);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        drop(inner);
        self.pump_queue();
        Ok(())
    }

    /// Starts a queued session now, ignoring the budget.
    pub fn start_now(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.queue.iter().any(|q| q == id) {
            bail!("session is not queued");
        }
        if inner.sessions.get(id).is_some_and(|s| s.run.is_some()) {
            bail!("the previous process is still stopping; try again in a moment");
        }
        inner.queue.retain(|q| q != id);
        self.launch(&mut inner, id);
        Ok(())
    }

    pub fn update(&self, id: &str, title: Option<String>, permission_mode: Option<String>) -> Result<SessionView> {
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        if let Some(t) = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            s.meta.title = t;
        }
        if let Some(m) = permission_mode {
            if !PERMISSION_MODES.contains(&m.as_str()) {
                bail!("unknown permission mode {m}");
            }
            if let Some(run) = &s.run {
                run.driver.set_permission_mode(&m);
            }
            s.meta.permission_mode = m;
        }
        s.meta.updated_ms = now_ms();
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        Ok(view(inner.sessions.get(id).unwrap(), &inner.queue))
    }

    pub fn remove(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.remove(id).context("no such session")?;
        inner.queue.retain(|q| q != id);
        if let Some(run) = &s.run {
            run.driver.kill();
        }
        self.store.remove(id);
        let _ = self.tx.send(UiEvent::Removed { session_id: id.to_string() });
        drop(inner);
        self.pump_queue();
        Ok(())
    }

    /// Stops every running session (daemon shutdown); they come back paused.
    pub fn shutdown(&self) {
        self.closing.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        for s in inner.sessions.values_mut() {
            if let Some(run) = &s.run {
                s.stopping = Some(if s.meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped });
                run.driver.kill();
            }
        }
    }

    // ------------------------------------------------------------ budget

    fn view_of(&self, id: &str) -> Option<SessionView> {
        let inner = self.inner.lock().unwrap();
        inner.sessions.get(id).map(|s| view(s, &inner.queue))
    }

    /// Starts queued sessions while the budget allows, oldest first.
    pub fn pump_queue(self: &Arc<Self>) {
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return; // the daemon is going away; queued sessions start next time
        }
        let mut inner = self.inner.lock().unwrap();
        // The next queued session whose previous process (if any) is gone.
        while let Some(pos) = inner.queue.iter().position(|q| inner.sessions.get(q).is_some_and(|s| s.run.is_none())) {
            let id = inner.queue[pos].clone();
            let (_, available) = procs::system_memory();
            let estimate = inner.budget.estimate();
            // Sessions that just started have not grown yet: hold memory for them.
            let ramping: u64 = inner.sessions.values().filter(|s| s.run.is_some()).map(|s| estimate.saturating_sub(s.memory.unwrap_or(0))).sum();
            let running = inner.sessions.values().filter(|s| s.run.is_some()).count();
            match inner.budget.admit(running, available.saturating_sub(ramping)) {
                Ok(()) => {
                    inner.queue.remove(pos);
                    self.launch(&mut inner, &id);
                }
                Err(reason) => {
                    let shortfall = (estimate + inner.budget.reserve_bytes).saturating_sub(available.saturating_sub(ramping));
                    let cap_hit = inner.budget.max_sessions > 0 && running >= inner.budget.max_sessions;
                    self.reclaim(&mut inner, if cap_hit { 0 } else { shortfall });
                    let detail = Some(reason);
                    if inner.sessions.get(&id).is_some_and(|s| s.meta.status != Status::Queued || s.meta.status_detail != detail) {
                        set_status(&mut inner, &id, Status::Queued, detail);
                        self.broadcast_session(&inner, &id);
                    }
                    break;
                }
            }
        }
    }

    /// Pauses idle sessions, longest idle first, to make room: one for a
    /// session cap (`need == 0`), or enough to cover `need` bytes. Does
    /// nothing while an earlier pause is still in progress, or if pausing
    /// every candidate would not be enough.
    fn reclaim(&self, inner: &mut Inner, need: u64) {
        if inner.sessions.values().any(|s| s.stopping.is_some()) {
            return;
        }
        let mut candidates: Vec<(Instant, String, u64)> = inner
            .sessions
            .values()
            .filter(|s| s.meta.status == Status::Idle && s.run.is_some() && s.meta.claude_session_id.is_some())
            .filter_map(|s| s.idle_since.filter(|t| t.elapsed() >= self.reclaim_after).map(|t| (t, s.meta.id.clone(), s.memory.unwrap_or(0))))
            .collect();
        candidates.sort();
        let mut chosen = Vec::new();
        let mut freed = 0u64;
        for (_, id, mem) in candidates {
            chosen.push(id);
            freed += mem;
            if freed >= need {
                break;
            }
        }
        if chosen.is_empty() || freed < need {
            return;
        }
        for id in chosen {
            tracing::info!(session = %id, "pausing an idle session to make room");
            self.hibernate(inner, &id);
        }
    }

    fn hibernate(&self, inner: &mut Inner, id: &str) {
        if let Some(s) = inner.sessions.get_mut(id)
            && let Some(run) = &s.run
        {
            s.stopping = Some(Status::Hibernated);
            run.driver.kill();
        }
    }

    fn launch(&self, inner: &mut Inner, id: &str) {
        let claude = self.claude_bin.lock().unwrap().clone().or_else(crate::agent::find_claude);
        let Some(s) = inner.sessions.get_mut(id) else { return };
        let Some(claude) = claude else {
            s.meta.status = Status::Error;
            s.meta.status_detail = Some("could not find the claude executable (set HANGAR_CLAUDE)".into());
            self.save(inner, id);
            self.broadcast_session(inner, id);
            return;
        };
        let resume = s.meta.claude_session_id.clone();
        let spec = claude::Launch {
            program: claude,
            cwd: PathBuf::from(&s.meta.cwd),
            model: s.meta.model.clone(),
            permission_mode: s.meta.permission_mode.clone(),
            resume: resume.clone(),
            session_id: id.to_string(),
        };
        match claude::Driver::spawn(spec, self.driver_tx.clone()) {
            Ok(driver) => {
                let outbox = std::mem::take(&mut s.meta.outbox);
                for text in &outbox {
                    driver.send_user(text);
                }
                s.run = Some(Run { driver, initialized: false, unconfirmed: outbox, cost_seen: 0.0, completed_turns: 0, resumed: resume.is_some() });
                s.stopping = None;
                s.idle_since = None;
                set_status(inner, id, Status::Starting, None);
            }
            Err(e) => set_status(inner, id, Status::Error, Some(format!("could not start claude: {e:#}"))),
        }
        self.save(inner, id);
        self.broadcast_session(inner, id);
    }

    // ------------------------------------------------------------ driver events

    async fn driver_loop(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<claude::DriverEvent>) {
        use claude::DriverEvent as E;
        while let Some(ev) = rx.recv().await {
            match ev {
                E::Message { session_id, pid, msg } => self.on_claude(&session_id, pid, msg),
                E::Permission { session_id, pid, request } => self.on_permission(&session_id, pid, request),
                E::PermissionCancelled { session_id, pid, request_id } => self.on_cancelled(&session_id, pid, &request_id),
                E::McpStatus { session_id, pid, servers } => {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(s) = inner.sessions.get_mut(&session_id).filter(|s| s.pid() == Some(pid)) {
                        s.mcp = servers;
                        self.broadcast_session(&inner, &session_id);
                    }
                }
                E::Exited { session_id, pid, code, stderr } => {
                    self.on_exit(&session_id, pid, code, stderr);
                    self.pump_queue();
                }
            }
        }
    }

    /// The session, if `pid` is its current process (older processes are ignored).
    fn current<'a>(inner: &'a mut Inner, id: &str, pid: u32) -> Option<&'a mut Session> {
        inner.sessions.get_mut(id).filter(|s| s.pid() == Some(pid))
    }

    fn on_claude(&self, id: &str, pid: u32, mut msg: Value) {
        let mut inner = self.inner.lock().unwrap();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let run = s.run.as_mut().expect("current session has a run");
        let kind = msg["type"].as_str().unwrap_or_default().to_string();
        let subtype = msg["subtype"].as_str().unwrap_or_default().to_string();
        let mut status = None;
        match (kind.as_str(), subtype.as_str()) {
            ("system", "init") => {
                run.initialized = true;
                run.unconfirmed.clear();
                if let Some(sid) = msg["session_id"].as_str() {
                    s.meta.claude_session_id = Some(sid.to_string());
                }
                if s.meta.model.is_none() {
                    s.meta.model = msg["model"].as_str().map(String::from);
                }
                // Modes can change inside Claude (plan mode exits, for one).
                if let Some(mode) = msg["permissionMode"].as_str().filter(|m| PERMISSION_MODES.contains(m)) {
                    s.meta.permission_mode = mode.to_string();
                }
                if let Some(list) = msg["mcp_servers"].as_array() {
                    s.mcp = list.iter().map(|m| json!({"name": m["name"], "status": m["status"]})).collect();
                }
                if s.meta.status == Status::Starting {
                    status = Some(Status::Working);
                }
            }
            ("assistant", _) => {
                if let Some(text) = first_text(&msg) {
                    s.meta.preview = truncate(&text, 160);
                }
                if matches!(s.meta.status, Status::Starting | Status::Idle) {
                    status = Some(Status::Working);
                }
            }
            ("result", _) => {
                // total_cost_usd is a running total for the process.
                let total = msg["total_cost_usd"].as_f64().unwrap_or(run.cost_seen);
                let turn_cost = if total >= run.cost_seen { total - run.cost_seen } else { total };
                run.cost_seen = total;
                run.completed_turns += 1;
                s.meta.cost_usd += turn_cost;
                msg["turn_cost_usd"] = json!(turn_cost);
                s.meta.turns += msg["num_turns"].as_u64().unwrap_or(0);
                if let Some(sid) = msg["session_id"].as_str() {
                    s.meta.claude_session_id = Some(sid.to_string());
                }
                if msg["is_error"].as_bool().unwrap_or(false) {
                    s.meta.preview = truncate(msg["result"].as_str().unwrap_or("the turn failed"), 160);
                }
                s.idle_since = Some(Instant::now());
                status = Some(Status::Idle);
            }
            _ => {}
        }
        let keep = match kind.as_str() {
            "stream_event" | "rate_limit_event" => false,
            "system" => matches!(subtype.as_str(), "init" | "compact_boundary"),
            _ => true,
        };
        if keep {
            self.push_event(&mut inner, id, EventBody::Claude { msg: slim(msg) });
        }
        if let Some(st) = status {
            let pending = inner.sessions.get(id).is_some_and(|s| !s.pending.is_empty());
            set_status(&mut inner, id, if pending { Status::NeedsYou } else { st }, None);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_permission(&self, id: &str, pid: u32, req: claude::PermissionRequest) {
        let mut inner = self.inner.lock().unwrap();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let can_remember = req.suggestions.as_ref().and_then(claude::session_only).is_some();
        s.pending.insert(
            req.request_id.clone(),
            PendingPermission {
                request_id: req.request_id.clone(),
                tool_name: req.tool_name.clone(),
                input: req.input.clone(),
                suggestions: req.suggestions.clone(),
                can_remember,
            },
        );
        self.push_event(
            &mut inner,
            id,
            EventBody::Permission {
                request_id: req.request_id,
                tool_name: req.tool_name,
                input: req.input,
                suggestions: req.suggestions,
                description: req.description,
            },
        );
        set_status(&mut inner, id, Status::NeedsYou, None);
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_cancelled(&self, id: &str, pid: u32, request_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        if s.pending.remove(request_id).is_none() {
            return;
        }
        let still_waiting = !s.pending.is_empty();
        let needs_you = s.meta.status == Status::NeedsYou;
        self.push_event(&mut inner, id, EventBody::Decision { request_id: request_id.into(), behavior: "cancelled".into() });
        if !still_waiting && needs_you {
            set_status(&mut inner, id, Status::Working, None);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_exit(&self, id: &str, pid: u32, code: Option<i32>, stderr: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let run = s.run.take().expect("current session has a run");
        s.pending.clear();
        s.memory = None;
        let peak = s.meta.peak_bytes;
        if !run.initialized && !run.unconfirmed.is_empty() {
            // Claude never started reading: those messages are still unsent.
            let mut unsent = run.unconfirmed;
            unsent.append(&mut s.meta.outbox);
            s.meta.outbox = unsent;
        }
        let tail = stderr.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" / ");
        let (status, detail) = match s.stopping.take() {
            Some(st) => (st, None),
            None if !run.initialized && run.resumed => {
                // A conversation that cannot be resumed would fail forever.
                s.meta.claude_session_id = None;
                let why = if tail.is_empty() { String::new() } else { format!(" ({tail})") };
                (Status::Error, Some(format!("could not resume the conversation{why}; your next message starts a new one")))
            }
            None if s.meta.status == Status::Idle => (if s.meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped }, None),
            None => {
                let detail = match code {
                    Some(c) => format!("claude exited with code {c}{}", if tail.is_empty() { String::new() } else { format!(": {tail}") }),
                    None => "claude stopped".into(),
                };
                (Status::Error, Some(detail))
            }
        };
        // Only full turns say anything about what a session costs.
        if run.completed_turns > 0 {
            inner.budget.record_peak(peak);
        }
        let queued = inner.queue.iter().any(|q| q == id);
        if status == Status::Error
            && let Some(d) = &detail
        {
            self.push_event(&mut inner, id, EventBody::Error { message: d.clone() });
        }
        // Someone sent a message while it was stopping: start it again.
        let has_outbox = inner.sessions.get(id).is_some_and(|s| !s.meta.outbox.is_empty());
        if queued && has_outbox && status != Status::Error {
            set_status(&mut inner, id, Status::Queued, None);
        } else {
            if status == Status::Error {
                inner.queue.retain(|q| q != id);
            }
            set_status(&mut inner, id, status, detail);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    /// Samples memory every 2 s, pauses long-idle sessions, re-checks the queue.
    async fn tick_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let snap = tokio::task::spawn_blocking(procs::snapshot).await.unwrap_or_default();
            {
                let mut inner = self.inner.lock().unwrap();
                let ids: Vec<String> = inner.sessions.keys().cloned().collect();
                for id in ids {
                    let s = inner.sessions.get_mut(&id).unwrap();
                    let Some(pid) = s.pid() else { continue };
                    let used = procs::Usage::of(procs::tree(&snap, pid)).private_bytes;
                    let changed = s.memory.is_none_or(|m| m.abs_diff(used) > (4 << 20));
                    s.memory = Some(used);
                    s.meta.peak_bytes = s.meta.peak_bytes.max(used);
                    let idle_too_long =
                        self.hibernate_after.is_some_and(|limit| s.meta.status == Status::Idle && s.idle_since.is_some_and(|t| t.elapsed() > limit));
                    if idle_too_long && s.meta.claude_session_id.is_some() && s.stopping.is_none() {
                        self.hibernate(&mut inner, &id);
                        continue;
                    }
                    if changed {
                        self.broadcast_session(&inner, &id);
                    }
                }
            }
            self.pump_queue();
        }
    }

    // ------------------------------------------------------------ plumbing

    fn push_event(&self, inner: &mut Inner, id: &str, body: EventBody) {
        let Some(s) = inner.sessions.get_mut(id) else { return };
        let ev = Event { seq: s.next_seq, ts: now_ms(), body };
        s.next_seq += 1;
        s.meta.updated_ms = ev.ts;
        self.store.append(id, &ev);
        s.events.push(ev.clone());
        if s.events.len() > MAX_EVENTS_IN_MEMORY {
            s.events.drain(..s.events.len() - MAX_EVENTS_IN_MEMORY);
        }
        let _ = self.tx.send(UiEvent::Event { session_id: id.to_string(), event: ev });
    }

    fn save(&self, inner: &Inner, id: &str) {
        if let Some(s) = inner.sessions.get(id) {
            self.store.save_meta(&s.meta);
        }
    }

    fn broadcast_session(&self, inner: &Inner, id: &str) {
        if let Some(s) = inner.sessions.get(id) {
            let _ = self.tx.send(UiEvent::Session { session: view(s, &inner.queue) });
        }
    }
}

fn set_status(inner: &mut Inner, id: &str, status: Status, detail: Option<String>) {
    if let Some(s) = inner.sessions.get_mut(id) {
        s.meta.status = status;
        s.meta.status_detail = detail;
        s.meta.updated_ms = now_ms();
    }
}

static VIEW_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn view(s: &Session, queue: &[String]) -> SessionView {
    SessionView {
        meta: s.meta.clone(),
        rev: VIEW_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        pid: s.pid(),
        memory_bytes: s.memory,
        pending: s.pending.values().cloned().collect(),
        queue_position: queue.iter().position(|q| *q == s.meta.id).map(|p| p + 1),
        mcp: s.mcp.clone(),
    }
}

fn first_text(msg: &Value) -> Option<String> {
    msg["message"]["content"].as_array()?.iter().find(|b| b["type"] == "text").and_then(|b| b["text"].as_str()).map(String::from)
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= n { s } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// Trims huge tool results so the event log stays small.
fn slim(mut msg: Value) -> Value {
    fn cut(v: &mut Value) {
        match v {
            Value::String(s) if s.len() > MAX_BLOCK_TEXT => {
                let mut end = MAX_BLOCK_TEXT;
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                let dropped = s.len() - end;
                s.truncate(end);
                s.push_str(&format!("\n… [{dropped} more bytes not shown]"));
            }
            Value::Array(a) => a.iter_mut().for_each(cut),
            Value::Object(o) => o.values_mut().for_each(cut),
            _ => {}
        }
    }
    if msg["type"] == "user" {
        cut(&mut msg["message"]);
        if let Some(o) = msg.as_object_mut() {
            o.remove("tool_use_result");
        }
    }
    msg
}

pub fn title_from(prompt: &str) -> String {
    let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("New session").trim();
    truncate(first, 60)
}

fn new_id() -> String {
    format!("s{}", crate::util::random_hex(6))
}

/// Session ids are `s` + 12 hex digits; anything else never reaches the disk.
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 13 && id.starts_with('s') && id[1..].chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_truncation() {
        assert_eq!(title_from("\n\n  Fix the login bug\nmore"), "Fix the login bug");
        assert_eq!(truncate("abc", 2), "ab…");
    }

    #[test]
    fn slim_cuts_big_tool_results_only() {
        let big = "x".repeat(MAX_BLOCK_TEXT * 2);
        let user = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": big}]}, "tool_use_result": {"stdout": "dup"}});
        let s = slim(user);
        let text = s["message"]["content"][0]["content"].as_str().unwrap();
        assert!(text.len() < MAX_BLOCK_TEXT + 100 && text.contains("more bytes not shown"));
        assert!(s.get("tool_use_result").is_none());
        let assistant = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "x".repeat(MAX_BLOCK_TEXT * 2)}]}});
        assert_eq!(slim(assistant.clone()), assistant, "assistant text is never cut");
    }

    #[test]
    fn ids_are_validated_before_touching_the_disk() {
        assert!(valid_id(&new_id()));
        assert!(!valid_id("../../etc"));
        assert!(!valid_id("s12345678901z"));
        assert!(!valid_id("x123456789012"));
    }

    #[test]
    fn status_serializes_snake_case() {
        assert_eq!(serde_json::to_value(Status::NeedsYou).unwrap(), "needs_you");
    }
}
