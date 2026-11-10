//! Agent sessions: start, watch, answer and pause many Claude Code sessions.
//!
//! Each session runs `claude` headless (stream-json in and out). Its events
//! are kept in `<home>/sessions/<id>/events.jsonl` and pushed to the UI.
//! The [`Budget`] decides when a session may start; sessions that cannot
//! start yet wait in a queue, and idle sessions can be hibernated (process
//! stopped, resumed with `--resume` on the next message) to make room.

pub mod claude;

use crate::budget::{Budget, Machine};
use crate::config::Config;
use crate::pool::upstream::now_ms;
use crate::procs;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
    User { text: String },
    /// A stream-json message from Claude Code (assistant, user/tool results, result, system).
    Claude { msg: Value },
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
    Decision { request_id: String, behavior: String },
    Status { status: Status, #[serde(default)] detail: Option<String> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingPermission {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
    pub suggestions: Option<Value>,
}

/// What the UI sees for one session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    #[serde(flatten)]
    pub meta: SessionMeta,
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
}

pub const PERMISSION_MODES: &[&str] = &["default", "acceptEdits", "plan", "bypassPermissions"];

struct Session {
    meta: SessionMeta,
    events: Vec<Event>,
    next_seq: u64,
    driver: Option<claude::Driver>,
    pending: BTreeMap<String, PendingPermission>,
    /// Messages waiting for the process (queued or hibernated sessions).
    outbox: Vec<String>,
    idle_since: Option<Instant>,
    memory: Option<u64>,
    mcp: Vec<Value>,
    /// Set when we stop the process on purpose.
    stopping: Option<Status>,
}

pub struct Manager {
    home: PathBuf,
    claude_bin: Mutex<Option<PathBuf>>,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<UiEvent>,
    driver_tx: mpsc::UnboundedSender<claude::DriverEvent>,
    hibernate_after: Option<Duration>,
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
        let hibernate_after = (config.budget.hibernate_after_mins > 0).then(|| Duration::from_secs(config.budget.hibernate_after_mins * 60));
        let manager = Arc::new(Manager {
            home: home.to_path_buf(),
            claude_bin: Mutex::new(None),
            inner: Mutex::new(Inner { sessions: HashMap::new(), queue: Vec::new(), budget: Budget::new(&config.budget) }),
            tx,
            driver_tx,
            hibernate_after,
        });
        manager.load();
        tokio::spawn(manager.clone().driver_loop(driver_rx));
        tokio::spawn(manager.clone().tick_loop());
        manager
    }

    pub fn set_claude_bin(&self, path: Option<PathBuf>) {
        *self.claude_bin.lock().unwrap() = path;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UiEvent> {
        self.tx.subscribe()
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.home.join("sessions").join(id)
    }

    fn load(&self) {
        let Ok(entries) = std::fs::read_dir(self.home.join("sessions")) else { return };
        let mut inner = self.inner.lock().unwrap();
        let mut queued = Vec::new();
        for e in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(e.path().join("meta.json")) else { continue };
            let Ok(mut meta) = serde_json::from_str::<SessionMeta>(&text) else { continue };
            // The processes are gone after a daemon restart.
            if meta.status.is_live() {
                meta.status = if meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped };
                meta.status_detail = None;
            }
            if meta.status == Status::Queued {
                queued.push((meta.created_ms, meta.id.clone()));
            }
            let next_seq = count_lines(&e.path().join("events.jsonl")) + 1;
            inner.sessions.insert(
                meta.id.clone(),
                Session { meta, events: Vec::new(), next_seq, driver: None, pending: BTreeMap::new(), outbox: Vec::new(), idle_since: None, memory: None, mcp: Vec::new(), stopping: None },
            );
        }
        queued.sort();
        inner.queue = queued.into_iter().map(|(_, id)| id).collect();
    }

    // ------------------------------------------------------------ queries

    pub fn list(&self) -> Vec<SessionView> {
        let inner = self.inner.lock().unwrap();
        let mut v: Vec<SessionView> = inner.sessions.values().map(|s| view(s, &inner.queue)).collect();
        v.sort_by(|a, b| b.meta.created_ms.cmp(&a.meta.created_ms));
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
            if let Some(first) = s.events.first() {
                if first.seq <= after + 1 {
                    return Ok(s.events.iter().filter(|e| e.seq > after).cloned().collect());
                }
            }
        }
        // Not (all) in memory: read the log.
        let text = std::fs::read_to_string(self.dir(id).join("events.jsonl")).unwrap_or_default();
        Ok(text.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.seq > after).collect())
    }

    pub fn recent_projects(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut seen: Vec<(u64, String)> = inner.sessions.values().map(|s| (s.meta.updated_ms, s.meta.cwd.clone())).collect();
        seen.sort_by(|a, b| b.0.cmp(&a.0));
        let mut out: Vec<String> = Vec::new();
        for (_, cwd) in seen {
            if !out.iter().any(|c| c.eq_ignore_ascii_case(&cwd)) {
                out.push(cwd);
            }
        }
        out
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
        };
        std::fs::create_dir_all(self.dir(&id))?;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.sessions.insert(
                id.clone(),
                Session { meta, events: Vec::new(), next_seq: 1, driver: None, pending: BTreeMap::new(), outbox: vec![prompt.clone()], idle_since: None, memory: None, mcp: Vec::new(), stopping: None },
            );
            self.push_event(&mut inner, &id, EventBody::User { text: prompt });
            inner.queue.push(id.clone());
            self.save_meta(&inner, &id);
        }
        self.pump_queue();
        Ok(self.view_of(&id).context("session vanished")?)
    }

    /// Sends a follow-up message, resuming the session if it is not running.
    pub fn send(self: &Arc<Self>, id: &str, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("empty message");
        }
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        match s.meta.status {
            Status::Queued => {
                s.outbox.push(text.to_string());
            }
            Status::Starting | Status::Working | Status::NeedsYou | Status::Idle if s.driver.is_some() => {
                s.driver.as_ref().unwrap().send_user(text);
                s.idle_since = None;
            }
            _ => {
                // Hibernated, stopped or failed: start again (resuming the conversation).
                s.outbox.push(text.to_string());
                if !inner.queue.iter().any(|q| q == id) {
                    inner.queue.push(id.to_string());
                }
                set_status(&mut inner, id, Status::Queued, None);
            }
        }
        self.push_event(&mut inner, id, EventBody::User { text: text.to_string() });
        let s = inner.sessions.get_mut(id).unwrap();
        if s.meta.status == Status::Idle {
            s.meta.status = Status::Working;
            s.meta.status_detail = None;
        }
        self.save_meta(&inner, id);
        self.broadcast_session(&inner, id);
        drop(inner);
        self.pump_queue();
        Ok(())
    }

    pub fn decide(&self, id: &str, request_id: &str, allow: bool, always: bool, message: Option<String>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let p = s.pending.remove(request_id).context("that request is no longer pending")?;
        let driver = s.driver.as_ref().context("session is not running")?;
        if allow {
            let updated = if always { p.suggestions.clone() } else { None };
            driver.allow(request_id, &p.input, updated);
        } else {
            driver.deny(request_id, message.as_deref().unwrap_or("The user declined this tool use."));
        }
        let still_waiting = !s.pending.is_empty();
        let behavior = if allow { if always { "allow_always" } else { "allow" } } else { "deny" };
        self.push_event(&mut inner, id, EventBody::Decision { request_id: request_id.into(), behavior: behavior.into() });
        if !still_waiting {
            set_status(&mut inner, id, Status::Working, None);
        }
        self.broadcast_session(&inner, id);
        Ok(())
    }

    pub fn interrupt(&self, id: &str) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        let s = inner.sessions.get(id).context("no such session")?;
        s.driver.as_ref().context("session is not running")?.interrupt();
        Ok(())
    }

    /// Stops the process. The conversation can be resumed later.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.retain(|q| q != id);
        let s = inner.sessions.get_mut(id).context("no such session")?;
        s.outbox.clear();
        if let Some(d) = &s.driver {
            s.stopping = Some(Status::Stopped);
            d.kill();
        } else {
            set_status(&mut inner, id, Status::Stopped, None);
        }
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
            if let Some(d) = &s.driver {
                d.set_permission_mode(&m);
            }
            s.meta.permission_mode = m;
        }
        s.meta.updated_ms = now_ms();
        self.save_meta(&inner, id);
        self.broadcast_session(&inner, id);
        Ok(view(inner.sessions.get(id).unwrap(), &inner.queue))
    }

    pub fn remove(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let s = inner.sessions.remove(id).context("no such session")?;
        inner.queue.retain(|q| q != id);
        if let Some(d) = &s.driver {
            d.kill();
        }
        let _ = std::fs::remove_dir_all(self.dir(id));
        let _ = self.tx.send(UiEvent::Removed { session_id: id.to_string() });
        drop(inner);
        self.pump_queue();
        Ok(())
    }

    /// Stops every running session (daemon shutdown).
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        for s in inner.sessions.values_mut() {
            if let Some(d) = &s.driver {
                s.stopping = Some(if s.meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped });
                d.kill();
            }
        }
    }

    // ------------------------------------------------------------ internals

    fn view_of(&self, id: &str) -> Option<SessionView> {
        let inner = self.inner.lock().unwrap();
        inner.sessions.get(id).map(|s| view(s, &inner.queue))
    }

    fn running(inner: &Inner) -> usize {
        inner.sessions.values().filter(|s| s.driver.is_some()).count()
    }

    /// Starts queued sessions while the budget allows, oldest first.
    pub fn pump_queue(self: &Arc<Self>) {
        let mut inner = self.inner.lock().unwrap();
        loop {
            let Some(id) = inner.queue.first().cloned() else { break };
            let (_, available) = procs::system_memory();
            match inner.budget.admit(Self::running(&inner), available) {
                Ok(()) => {
                    inner.queue.remove(0);
                    self.launch(&mut inner, &id);
                }
                Err(reason) => {
                    // Make room by hibernating the longest-idle session, if any.
                    let victim = inner
                        .sessions
                        .values()
                        .filter(|s| s.meta.status == Status::Idle && s.driver.is_some() && s.meta.claude_session_id.is_some() && s.stopping.is_none())
                        .filter(|s| s.idle_since.is_some_and(|t| t.elapsed() > Duration::from_secs(30)))
                        .min_by_key(|s| s.idle_since)
                        .map(|s| s.meta.id.clone());
                    if let Some(v) = victim {
                        tracing::info!(session = %v, "hibernating an idle session to make room");
                        self.hibernate(&mut inner, &v);
                    }
                    let detail = Some(reason);
                    let changed = inner.sessions.get(&id).is_some_and(|s| s.meta.status_detail != detail);
                    if changed {
                        set_status(&mut inner, &id, Status::Queued, detail);
                        self.broadcast_session(&inner, &id);
                    }
                    break;
                }
            }
        }
    }

    fn hibernate(&self, inner: &mut Inner, id: &str) {
        if let Some(s) = inner.sessions.get_mut(id) {
            if let Some(d) = &s.driver {
                s.stopping = Some(Status::Hibernated);
                d.kill();
            }
        }
    }

    fn launch(self: &Arc<Self>, inner: &mut Inner, id: &str) {
        let claude = self.claude_bin.lock().unwrap().clone().or_else(crate::agent::find_claude);
        let Some(s) = inner.sessions.get_mut(id) else { return };
        let Some(claude) = claude else {
            s.meta.status = Status::Error;
            s.meta.status_detail = Some("could not find the claude executable (set HANGAR_CLAUDE)".into());
            self.save_meta(inner, id);
            self.broadcast_session(inner, id);
            return;
        };
        let outbox = std::mem::take(&mut s.outbox);
        let spec = claude::Launch {
            program: claude,
            cwd: PathBuf::from(&s.meta.cwd),
            model: s.meta.model.clone(),
            permission_mode: s.meta.permission_mode.clone(),
            resume: s.meta.claude_session_id.clone(),
            session_id: id.to_string(),
        };
        match claude::Driver::spawn(spec, self.driver_tx.clone()) {
            Ok(driver) => {
                for text in outbox {
                    driver.send_user(&text);
                }
                s.driver = Some(driver);
                s.stopping = None;
                s.idle_since = None;
                set_status(inner, id, Status::Starting, None);
            }
            Err(e) => {
                set_status(inner, id, Status::Error, Some(format!("could not start claude: {e:#}")));
            }
        }
        self.broadcast_session(inner, id);
    }

    async fn driver_loop(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<claude::DriverEvent>) {
        while let Some(ev) = rx.recv().await {
            match ev {
                claude::DriverEvent::Message { session_id, msg } => self.on_claude(&session_id, msg),
                claude::DriverEvent::Permission { session_id, request } => self.on_permission(&session_id, request),
                claude::DriverEvent::McpStatus { session_id, servers } => {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(s) = inner.sessions.get_mut(&session_id) {
                        s.mcp = servers;
                    }
                    self.broadcast_session(&inner, &session_id);
                }
                claude::DriverEvent::Exited { session_id, pid, code, stderr } => {
                    self.on_exit(&session_id, pid, code, stderr);
                    self.pump_queue();
                }
            }
        }
    }

    fn on_claude(&self, id: &str, msg: Value) {
        let mut inner = self.inner.lock().unwrap();
        let Some(s) = inner.sessions.get_mut(id) else { return };
        let kind = msg["type"].as_str().unwrap_or_default().to_string();
        let subtype = msg["subtype"].as_str().unwrap_or_default().to_string();
        let mut status = None;
        match (kind.as_str(), subtype.as_str()) {
            ("system", "init") => {
                if let Some(sid) = msg["session_id"].as_str() {
                    s.meta.claude_session_id = Some(sid.to_string());
                }
                if s.meta.model.is_none() {
                    s.meta.model = msg["model"].as_str().map(String::from);
                }
                if let Some(list) = msg["mcp_servers"].as_array() {
                    s.mcp = list.clone();
                }
                if s.meta.status == Status::Starting {
                    status = Some(Status::Working);
                }
            }
            ("system", _) | ("rate_limit_event", _) | ("stream_event", _) => {}
            ("assistant", _) => {
                if let Some(text) = first_text(&msg) {
                    s.meta.preview = truncate(&text, 160);
                }
                if matches!(s.meta.status, Status::Starting | Status::Idle) {
                    status = Some(Status::Working);
                }
            }
            ("result", _) => {
                if let Some(c) = msg["total_cost_usd"].as_f64() {
                    s.meta.cost_usd += c;
                }
                s.meta.turns += msg["num_turns"].as_u64().unwrap_or(0);
                if let Some(sid) = msg["session_id"].as_str() {
                    s.meta.claude_session_id = Some(sid.to_string());
                }
                let failed = msg["is_error"].as_bool().unwrap_or(false);
                if failed {
                    let text = msg["result"].as_str().unwrap_or("the turn failed").to_string();
                    s.meta.preview = truncate(&text, 160);
                }
                s.idle_since = Some(Instant::now());
                status = Some(Status::Idle);
            }
            _ => {}
        }
        let important = !matches!(kind.as_str(), "stream_event" | "rate_limit_event") && !(kind == "system" && subtype != "init" && subtype != "compact_boundary");
        if important {
            self.push_event(&mut inner, id, EventBody::Claude { msg: slim(msg) });
        }
        if let Some(st) = status {
            let pending = inner.sessions.get(id).is_some_and(|s| !s.pending.is_empty());
            set_status(&mut inner, id, if pending { Status::NeedsYou } else { st }, None);
        }
        self.save_meta(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_permission(&self, id: &str, req: claude::PermissionRequest) {
        let mut inner = self.inner.lock().unwrap();
        let Some(s) = inner.sessions.get_mut(id) else { return };
        s.pending.insert(
            req.request_id.clone(),
            PendingPermission { request_id: req.request_id.clone(), tool_name: req.tool_name.clone(), input: req.input.clone(), suggestions: req.suggestions.clone() },
        );
        self.push_event(
            &mut inner,
            id,
            EventBody::Permission { request_id: req.request_id, tool_name: req.tool_name, input: req.input, suggestions: req.suggestions, description: req.description },
        );
        set_status(&mut inner, id, Status::NeedsYou, None);
        self.save_meta(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_exit(&self, id: &str, pid: u32, code: Option<i32>, stderr: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        let peak;
        {
            let Some(s) = inner.sessions.get_mut(id) else { return };
            if s.driver.as_ref().map(|d| d.pid) != Some(pid) {
                return; // an older process of this session
            }
            s.driver = None;
            s.pending.clear();
            s.memory = None;
            peak = s.meta.peak_bytes;
        }
        inner.budget.record_peak(peak);
        let s = inner.sessions.get_mut(id).unwrap();
        let (status, detail) = match s.stopping.take() {
            Some(st) => (st, None),
            None if s.meta.status == Status::Idle => (if s.meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped }, None),
            None => {
                let tail = stderr.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" / ");
                let detail = match code {
                    Some(c) => format!("claude exited with code {c}{}", if tail.is_empty() { String::new() } else { format!(": {tail}") }),
                    None => "claude stopped".into(),
                };
                (Status::Error, Some(detail))
            }
        };
        if status == Status::Error {
            if let Some(d) = &detail {
                self.push_event(&mut inner, id, EventBody::Error { message: d.clone() });
            }
        }
        set_status(&mut inner, id, status, detail);
        self.save_meta(&inner, id);
        self.broadcast_session(&inner, id);
    }

    /// Samples memory every 2 s, hibernates long-idle sessions, re-checks the queue.
    async fn tick_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let snap = procs::snapshot();
            {
                let mut inner = self.inner.lock().unwrap();
                let ids: Vec<String> = inner.sessions.keys().cloned().collect();
                for id in ids {
                    let s = inner.sessions.get_mut(&id).unwrap();
                    let Some(pid) = s.driver.as_ref().map(|d| d.pid) else { continue };
                    let used = procs::Usage::of(procs::tree(&snap, pid)).private_bytes;
                    let changed = s.memory.map_or(true, |m| m.abs_diff(used) > (4 << 20));
                    s.memory = Some(used);
                    s.meta.peak_bytes = s.meta.peak_bytes.max(used);
                    if let Some(limit) = self.hibernate_after {
                        if s.meta.status == Status::Idle && s.idle_since.is_some_and(|t| t.elapsed() > limit) && s.meta.claude_session_id.is_some() {
                            self.hibernate(&mut inner, &id);
                            continue;
                        }
                    }
                    if changed {
                        self.broadcast_session(&inner, &id);
                    }
                }
            }
            self.pump_queue();
        }
    }

    fn push_event(&self, inner: &mut Inner, id: &str, body: EventBody) {
        let Some(s) = inner.sessions.get_mut(id) else { return };
        let ev = Event { seq: s.next_seq, ts: now_ms(), body };
        s.next_seq += 1;
        s.meta.updated_ms = ev.ts;
        if let Ok(line) = serde_json::to_string(&ev) {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir(id).join("events.jsonl")) {
                let _ = writeln!(f, "{line}");
            }
        }
        s.events.push(ev.clone());
        if s.events.len() > MAX_EVENTS_IN_MEMORY {
            s.events.drain(..s.events.len() - MAX_EVENTS_IN_MEMORY);
        }
        let _ = self.tx.send(UiEvent::Event { session_id: id.to_string(), event: ev });
    }

    fn save_meta(&self, inner: &Inner, id: &str) {
        if let Some(s) = inner.sessions.get(id) {
            let path = self.dir(id).join("meta.json");
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, serde_json::to_vec_pretty(&s.meta).unwrap_or_default()).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
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
        if s.meta.status != status {
            tracing::debug!(session = id, ?status, "status");
        }
        s.meta.status = status;
        s.meta.status_detail = detail;
        s.meta.updated_ms = now_ms();
    }
}

fn view(s: &Session, queue: &[String]) -> SessionView {
    SessionView {
        meta: s.meta.clone(),
        pid: s.driver.as_ref().map(|d| d.pid),
        memory_bytes: s.memory,
        pending: s.pending.values().cloned().collect(),
        queue_position: queue.iter().position(|q| *q == s.meta.id).map(|p| p + 1),
        mcp: s.mcp.iter().map(|m| json!({"name": m["name"], "status": m["status"]})).collect(),
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
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).expect("OS randomness");
    format!("s{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

fn count_lines(path: &Path) -> u64 {
    std::fs::read_to_string(path).map(|t| t.lines().count() as u64).unwrap_or(0)
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
    fn status_serializes_snake_case() {
        assert_eq!(serde_json::to_value(Status::NeedsYou).unwrap(), "needs_you");
    }
}
