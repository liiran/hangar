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
pub mod recent;
mod store;
pub mod trust;

use crate::budget::{Budget, Machine};
use crate::config::Config;
use crate::procs;
use crate::util::Locked;
use crate::util::now_ms;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::Store;
use tokio::sync::{broadcast, mpsc, oneshot};

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
    /// hangar's id for each message in `outbox`, in the same order.
    #[serde(default)]
    pub outbox_ids: Vec<String>,
    /// How full the context window was when Claude last said.
    #[serde(default)]
    pub context: Option<ContextUsage>,
}

/// How full Claude's context window is, in Claude Code's own numbers (the
/// ones its `/context` command shows). Nothing here is worked out by hangar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub used_tokens: u64,
    pub max_tokens: u64,
    /// Claude Code's own whole-number percentage.
    pub percent: u8,
    #[serde(default)]
    pub model: Option<String>,
    /// Where Claude Code summarizes the conversation by itself, when it does.
    #[serde(default)]
    pub auto_compact_at: Option<u64>,
    /// What the used part is made of.
    #[serde(default)]
    pub parts: Vec<ContextPart>,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPart {
    pub name: String,
    pub tokens: u64,
}

impl ContextUsage {
    /// Reads Claude Code's answer to `get_context_usage`. Without its own
    /// percentage there is nothing to show: hangar does not make one up.
    fn read(v: &Value, at_ms: u64) -> Option<ContextUsage> {
        let max_tokens = v["maxTokens"].as_u64().filter(|m| *m > 0)?;
        let used_tokens = v["totalTokens"].as_u64()?;
        let percent = v["percentage"].as_f64().filter(|p| p.is_finite())?.clamp(0.0, 100.0).round() as u8;
        let parts = v["categories"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter(|c| c["kind"] == "used")
                    .filter_map(|c| Some(ContextPart { name: label(c["name"].as_str()?, 48)?, tokens: c["tokens"].as_u64().filter(|t| *t > 0)? }))
                    .take(16)
                    .collect()
            })
            .unwrap_or_default();
        Some(ContextUsage {
            used_tokens,
            max_tokens,
            percent,
            model: v["model"].as_str().and_then(|m| label(m, 100)),
            auto_compact_at: v["autoCompactThreshold"].as_u64().filter(|_| v["isAutoCompactEnabled"] == true),
            parts,
            at_ms,
        })
    }
}

/// Your plan's usage limits, as Claude Code last reported them. They belong
/// to the account, not to one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub windows: Vec<LimitWindow>,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// Claude Code's name for the window: `five_hour`, `seven_day`.
    pub name: String,
    /// The share used: 0.46 is 46%.
    pub used: f64,
    /// When the window starts over, in seconds since 1970.
    #[serde(default)]
    pub resets_at: Option<u64>,
    /// `allowed`, `allowed_warning` or `rejected`.
    pub status: String,
}

impl Limits {
    /// Reads the `rate_limit_info` of a `rate_limit_event`. `None` when it
    /// names no window with a usage figure (an API key has none).
    fn read(info: &Value, at_ms: u64) -> Option<Limits> {
        fn window(name: &str, w: &Value) -> Option<LimitWindow> {
            let name = Some(name).filter(|n| !n.is_empty() && n.len() <= 32 && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))?;
            let used = w["utilization"].as_f64().filter(|u| u.is_finite() && *u >= 0.0)?.min(10.0);
            let status = w["status"].as_str().filter(|s| ["allowed", "allowed_warning", "rejected"].contains(s)).unwrap_or("allowed");
            // A time between 2001 and 5138: anything else is not a time in seconds.
            let resets_at = w["resetsAt"].as_u64().filter(|t| (1_000_000_000..100_000_000_000).contains(t));
            Some(LimitWindow { name: name.to_string(), used, resets_at, status: status.to_string() })
        }
        let mut windows: Vec<LimitWindow> = match info["unifiedWindows"].as_object() {
            Some(all) => all.iter().filter_map(|(name, w)| window(name, w)).collect(),
            // Older versions name one window only.
            None => info["rateLimitType"].as_str().and_then(|name| window(name, info)).into_iter().collect(),
        };
        // The short window first, then the long one, then any others.
        let rank = |w: &LimitWindow| (["five_hour", "seven_day"].iter().position(|n| *n == w.name).unwrap_or(2), w.name.clone());
        windows.sort_by_key(rank);
        windows.truncate(6);
        (!windows.is_empty()).then_some(Limits { windows, at_ms })
    }
}

/// A short name from Claude Code, safe to show: no control or invisible
/// characters, and not longer than `max`.
fn label(text: &str, max: usize) -> Option<String> {
    let clean: String = crate::util::printable(text).trim().chars().take(max).collect();
    (!clean.is_empty()).then_some(clean)
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
    /// A message the user sent. `uuid` is hangar's id for it, which Claude
    /// Code files its checkpoint under: a message that has one can be
    /// rewound to.
    User {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uuid: Option<String>,
        /// The permission mode the session was in when it was sent: going
        /// back to before the message puts that mode back.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
    /// The user went back to before their message number `to` (Claude
    /// Code's rewind). `code` says what was put back in the files, if they
    /// were; `conversation` that the message and all after it are gone from
    /// what Claude sees.
    Rewind {
        to: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<Value>,
        #[serde(default)]
        conversation: bool,
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
        /// The helper agent that asks, when it is not Claude itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// `allow`, `allow_always` (rules remembered for this session), `deny` or `cancelled`.
    /// `detail` is what the user chose beyond that: the answers to a
    /// question, the mode a plan continues in, or what they asked to change.
    Decision {
        request_id: String,
        behavior: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
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
    /// The helper agent that asks, when it is not Claude itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// A helper agent Claude started with its Agent tool, while it runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Helper {
    /// Claude Code's id for the task; a helper's requests carry it as `agent_id`.
    pub id: String,
    /// The Agent tool call that started it. Empty if Claude Code never said.
    pub tool_use_id: String,
    pub description: String,
    /// Runs beside the conversation: Claude's turn can end while it works.
    pub background: bool,
    /// 1 for a helper Claude started, 2 for one that a helper started.
    #[serde(skip)]
    depth: u64,
}

/// How a helper ended.
struct Ended {
    /// It was stopped, not finished.
    stopped: bool,
    background: bool,
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
    /// The helper agents running right now. Empty when the process is not running.
    pub agents: Vec<Helper>,
    /// A turn of the conversation itself is running. A session can be
    /// working without one: its helpers work on after Claude's turn.
    pub in_turn: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    Session {
        session: Box<SessionView>,
    },
    Event {
        session_id: String,
        event: Box<Event>,
    },
    Removed {
        session_id: String,
    },
    /// The daemon is about to stop on purpose (`hangar stop`, or "Stop hangar" in the app).
    Stopping,
}

/// How the user answered a permission request.
#[derive(Debug, Default, Clone)]
pub struct Answer {
    pub allow: bool,
    /// Allow: also remember Claude's suggested rules for this session.
    pub always: bool,
    /// Deny: the words Claude reads, instead of the usual ones.
    pub message: Option<String>,
    /// A question (`AskUserQuestion`), allowed: the answer to each question, by its text.
    pub answers: Option<BTreeMap<String, String>>,
    /// A plan (`ExitPlanMode`), allowed: the permission mode to carry on in.
    pub mode: Option<String>,
    /// What the user typed, in their own words. Deny: what they want done
    /// differently. A plan, allowed: a note that goes to Claude with the
    /// approval.
    pub feedback: Option<String>,
    /// Deny: end the turn too, so Claude waits for the next message.
    pub stop: bool,
    /// A question, denied: the user wants to talk the questions over first.
    pub clarify: bool,
}

/// The tool Claude Code asks questions with.
pub const ASK_TOOL: &str = "AskUserQuestion";
/// The tool Claude Code presents a plan with.
pub const PLAN_TOOL: &str = "ExitPlanMode";
/// The modes a session can carry on in once a plan is approved.
pub const AFTER_PLAN: &[&str] = &["acceptEdits", "default"];
/// The longest answer or feedback sent on to Claude, in characters.
const MAX_REPLY: usize = 8000;

// Claude Code's own wording for a refused tool call, so that Claude reads a
// refusal from hangar exactly as one from its terminal.
const REFUSED: &str =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). ";
const REFUSED_STOP: &str = "STOP what you are doing and wait for the user to tell you how to proceed.";
const REFUSED_BECAUSE: &str = "To tell you how to proceed, the user said:\n";

/// What goes back to Claude for one answered request.
#[derive(Debug, PartialEq)]
struct Reply {
    allow: bool,
    /// Allow: the input the tool runs with.
    input: Value,
    /// Allow: permission changes applied with it.
    permissions: Option<Value>,
    /// Deny: what Claude reads.
    message: String,
    /// Deny: the turn ends too.
    interrupt: bool,
    /// For the history: `allow`, `allow_always` or `deny`.
    behavior: &'static str,
    detail: Option<Value>,
    /// The permission mode the session is in afterwards, if it changes.
    mode: Option<String>,
    /// A plan, allowed: the user's note, which Claude reads with the approval.
    note: Option<String>,
}

/// Works out the reply to `p` from the user's answer, or says why the
/// answer does not fit the request. Nothing is sent here.
fn reply_to(p: &PendingPermission, a: &Answer) -> Result<Reply> {
    let reply = |text: &str| -> String { text.trim().chars().take(MAX_REPLY).collect() };
    let mut r = Reply {
        allow: a.allow,
        input: p.input.clone(),
        permissions: None,
        message: String::new(),
        interrupt: false,
        behavior: "deny",
        detail: None,
        mode: None,
        note: None,
    };
    if a.answers.is_some() && !(a.allow && p.tool_name == ASK_TOOL) {
        bail!("answers go with allowing a question");
    }
    if a.mode.is_some() && !(a.allow && p.tool_name == PLAN_TOOL) {
        bail!("a mode goes with approving a plan");
    }
    if a.clarify && (a.allow || p.tool_name != ASK_TOOL) {
        bail!("only a question can be talked over");
    }
    let feedback = a.feedback.as_deref().map(reply).filter(|f| !f.is_empty());
    if a.allow && feedback.is_some() && p.tool_name != PLAN_TOOL {
        bail!("a note goes with approving a plan");
    }
    if a.allow {
        r.behavior = "allow";
        if p.tool_name == ASK_TOOL {
            // Allowed with no answers, Claude is told "the user did not
            // answer": that is never what an Allow click means.
            let asked: Vec<&str> = p.input["questions"].as_array().map(|qs| qs.iter().filter_map(|q| q["question"].as_str()).collect()).unwrap_or_default();
            let mut answers = serde_json::Map::new();
            for (question, answer) in a.answers.iter().flatten() {
                if !asked.contains(&question.as_str()) {
                    bail!("that is not one of the questions Claude asked");
                }
                let answer = reply(answer);
                if !answer.is_empty() {
                    answers.insert(question.clone(), json!(answer));
                }
            }
            if answers.is_empty() {
                bail!("choose an answer first");
            }
            r.input["answers"] = Value::Object(answers.clone());
            r.detail = Some(json!({"answers": answers}));
        } else if p.tool_name == PLAN_TOOL {
            if let Some(mode) = &a.mode {
                if !AFTER_PLAN.contains(&mode.as_str()) {
                    bail!("a plan can continue in acceptEdits or default mode");
                }
                r.permissions = Some(json!([{"type": "setMode", "mode": mode, "destination": "session"}]));
                r.detail = Some(json!({"mode": mode}));
                r.mode = Some(mode.clone());
            }
            if let Some(note) = feedback {
                r.detail.get_or_insert_with(|| json!({}))["feedback"] = json!(note);
                r.note = Some(note);
            }
        } else if a.always
            && let Some(rules) = p.suggestions.as_ref().and_then(claude::session_only)
        {
            r.permissions = Some(rules);
            r.behavior = "allow_always";
        }
        return Ok(r);
    }
    if a.clarify {
        let asked: Vec<String> = p.input["questions"]
            .as_array()
            .map(|qs| qs.iter().filter_map(|q| q["question"].as_str()).map(|q| format!("- {q}\n  (No answer provided)")).collect())
            .unwrap_or_default();
        r.message = format!(
            "The user wants to clarify these questions.\nThis means they may have additional information, context or questions for you.\nTake their response into account and then reformulate the questions if appropriate.\nStart by asking them what they would like to clarify.\n\nQuestions asked:\n{}",
            asked.join("\n")
        );
        r.detail = Some(json!({"clarify": true}));
    } else if let Some(feedback) = feedback {
        r.message = format!("{REFUSED}{REFUSED_BECAUSE}{feedback}");
        r.detail = Some(json!({"feedback": feedback}));
    } else if a.stop {
        r.message = format!("{REFUSED}{REFUSED_STOP}");
        r.interrupt = true;
        r.detail = Some(json!({"stopped": true}));
    } else {
        r.message = a.message.as_deref().map(reply).filter(|m| !m.is_empty()).unwrap_or_else(|| "The user declined this tool use.".into());
    }
    Ok(r)
}

pub struct NewSession {
    pub cwd: String,
    pub prompt: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    /// The user confirmed they trust the folder.
    pub trust: bool,
    /// The fingerprint of the trust notice the user saw. If the folder's
    /// settings changed since, the confirmation does not count.
    pub trust_seen: Option<String>,
    /// `cwd` is a new folder to make first (its parent must exist).
    pub create_folder: bool,
}

/// What rewinding to one of the user's messages needs (see `Manager::rewind_target`).
struct RewindTarget {
    uuid: String,
    newest: Option<String>,
    mode: Option<String>,
}

/// What to do with Claude Code's rewind for a message (see [`Manager::rewind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rewind {
    /// Only say what restoring the code would change.
    Preview,
    Code,
    Conversation,
    Both,
}

/// Changes to a session; `None` leaves a field as it is.
#[derive(Default)]
pub struct Update {
    pub title: Option<String>,
    pub permission_mode: Option<String>,
    /// An empty name means Claude Code's default model.
    pub model: Option<String>,
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

/// Why a session does not start again in a folder whose settings are not the ones agreed to.
pub const CONFIRM_FOLDER: &str = "This folder's Claude settings are not the ones you agreed to: they changed, or hangar has not shown them to you yet. Start a new session in this folder to see them. After that, this session can continue";

/// The running Claude process of a session.
struct Run {
    driver: claude::Driver,
    /// Claude reported `system/init`: it is up and reading messages.
    initialized: bool,
    /// Messages written before `initialized` (id and text); re-sent if the process dies first.
    unconfirmed: Vec<(String, String)>,
    /// Requests to Claude Code whose answers someone waits for, by request id.
    waiting: HashMap<String, oneshot::Sender<Result<Value, String>>>,
    asked: u64,
    /// Last cumulative `total_cost_usd` reported by this process.
    cost_seen: f64,
    completed_turns: u64,
    resumed: bool,
    /// When Claude was last asked how full its context is, while unanswered.
    context_asked: Option<Instant>,
    /// Something changed since that question: ask again once it is answered.
    context_stale: bool,
    /// The helper agents running right now.
    helpers: Vec<Helper>,
    /// Every helper this process has started, by task id: their later
    /// messages are kept, those of other background tasks are not.
    helper_ids: BTreeSet<String>,
    /// A turn of the conversation itself is running (from `init` to `result`).
    turn: bool,
    /// The last helper ended with no turn running. Claude Code then starts a
    /// turn by itself, in which Claude reads the helper's report or hears
    /// that it was stopped; if it has not by `SETTLE`, the session counts
    /// as idle.
    settle: Option<Instant>,
    /// When the user last stopped the session's turn. Helpers that end
    /// because of that are not followed by a turn.
    interrupted: Option<Instant>,
    /// The helpers the user stopped one by one. Each is followed by a turn
    /// in which Claude hears of it, whatever else was stopped around then.
    stopped_alone: BTreeSet<String>,
    /// A helper in the background ended while a turn was running: Claude
    /// Code starts the turn about it once that one has ended.
    report_due: bool,
}

/// How long a session stays "working" after its last helper ended, waiting
/// for the turn in which Claude reads the report.
const SETTLE: Duration = Duration::from_secs(5);

impl Run {
    /// Asks Claude how full its context is. One question is out at a time:
    /// while it is unanswered, a change is only noted, and asked about when
    /// the answer comes (see `on_context`). An answer that never comes
    /// stops counting after 10 s.
    /// Nothing is running: no turn and no helper.
    fn at_rest(&self) -> bool {
        !self.turn && self.helpers.is_empty()
    }

    /// The user has just stopped everything: no turn is coming.
    fn just_interrupted(&self) -> bool {
        self.interrupted.is_some_and(|t| t.elapsed() < Duration::from_secs(10))
    }

    /// Notes what a task message says about a helper. Returns how a helper
    /// ended, if the message says that one did.
    fn track(&mut self, subtype: &str, msg: &Value) -> Option<Ended> {
        let id = msg["task_id"].as_str().unwrap_or_default();
        match subtype {
            "task_started" if msg["task_type"] == "local_agent" && !id.is_empty() && id.len() <= 64 => {
                self.helper_ids.insert(id.to_string());
                self.helpers.retain(|h| h.id != id);
                self.helpers.push(Helper {
                    id: id.to_string(),
                    tool_use_id: msg["tool_use_id"].as_str().unwrap_or_default().to_string(),
                    description: truncate(msg["description"].as_str().unwrap_or_default(), 200),
                    background: msg["is_backgrounded"] == true,
                    depth: msg["spawn_depth"].as_u64().unwrap_or(1),
                });
                self.settle = None;
                None
            }
            // `task_updated` reports every change; only an end counts here.
            "task_updated" | "task_notification" => {
                let status = if subtype == "task_updated" { &msg["patch"]["status"] } else { &msg["status"] };
                let status = status.as_str().unwrap_or_default();
                if matches!(status, "" | "running" | "pending") {
                    return None;
                }
                let at = self.helpers.iter().position(|h| h.id == id)?;
                let helper = self.helpers.remove(at);
                Some(Ended { stopped: matches!(status, "killed" | "stopped"), background: helper.background })
            }
            // The whole list of what runs in the background, whenever it changes.
            "background_tasks_changed" => {
                let listed: Vec<&Value> = msg["tasks"].as_array().map(|a| a.iter().filter(|t| t["task_type"] == "local_agent").collect()).unwrap_or_default();
                let before = self.helpers.len();
                self.helpers.retain(|h| !h.background || listed.iter().any(|t| t["task_id"] == h.id.as_str()));
                // With nothing in the background and no turn running, nothing
                // can be running: a helper inside a helper has no turn to run in.
                if listed.is_empty() && !self.turn {
                    self.helpers.clear();
                }
                let ended = self.helpers.len() < before;
                for t in listed {
                    let Some(id) = t["task_id"].as_str().filter(|i| !i.is_empty() && i.len() <= 64) else { continue };
                    match self.helpers.iter_mut().find(|h| h.id == id) {
                        // A helper that started in the foreground and was moved to the background.
                        Some(h) => h.background = true,
                        None => {
                            self.helper_ids.insert(id.to_string());
                            self.helpers.push(Helper {
                                id: id.to_string(),
                                tool_use_id: String::new(),
                                description: truncate(t["description"].as_str().unwrap_or_default(), 200),
                                background: true,
                                depth: 1,
                            });
                        }
                    }
                }
                ended.then_some(Ended { stopped: false, background: true })
            }
            _ => None,
        }
    }

    fn ask_context(&mut self) {
        if self.context_asked.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
            self.context_stale = true;
            return;
        }
        self.driver.context_usage();
        self.context_asked = Some(Instant::now());
        self.context_stale = false;
    }
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
    /// What rewinding to a message would put back in the files, as last
    /// previewed: the message's `seq` and Claude Code's answer.
    preview: Option<(u64, Value)>,
}

impl Session {
    fn new(meta: SessionMeta, next_seq: u64) -> Session {
        Session {
            meta,
            events: Vec::new(),
            next_seq,
            run: None,
            pending: BTreeMap::new(),
            idle_since: None,
            memory: None,
            mcp: Vec::new(),
            stopping: None,
            preview: None,
        }
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
    /// What `unshared_servers` found, and the config's (modified time, size) then.
    unshared: Mutex<Option<(ConfigStamp, Vec<crate::claude_config::Unshared>)>>,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<UiEvent>,
    driver_tx: mpsc::UnboundedSender<claude::DriverEvent>,
    hibernate_after: Option<Duration>,
    reclaim_after: Duration,
    /// Set by `shutdown`: start nothing new.
    closing: std::sync::atomic::AtomicBool,
    /// The plan's usage limits, from the last session that reported them.
    limits: Mutex<Option<Limits>>,
}

type ConfigStamp = Option<(Option<std::time::SystemTime>, u64)>;

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
            unshared: Mutex::new(None),
            inner: Mutex::new(Inner { sessions: HashMap::new(), queue: Vec::new(), budget: Budget::new(b) }),
            tx,
            driver_tx,
            hibernate_after: (b.hibernate_after_mins > 0).then(|| Duration::from_secs(b.hibernate_after_mins * 60)),
            reclaim_after: Duration::from_secs(b.reclaim_idle_after_secs),
            closing: std::sync::atomic::AtomicBool::new(false),
            limits: Mutex::new(std::fs::read(home.join(LIMITS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())),
        });
        manager.load();
        tokio::spawn(manager.clone().driver_loop(driver_rx));
        tokio::spawn(manager.clone().tick_loop());
        manager
    }

    pub fn set_claude_bin(&self, path: Option<PathBuf>) {
        *self.claude_bin.locked() = path;
    }

    /// Where Claude Code's own trust decisions are read from (tests point it elsewhere).
    pub fn set_claude_json(&self, path: PathBuf) {
        *self.claude_json.locked() = path;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UiEvent> {
        self.tx.subscribe()
    }

    fn load(&self) {
        let mut inner = self.inner.locked();
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
        let inner = self.inner.locked();
        let mut v: Vec<SessionView> = inner.sessions.values().map(|s| view(s, &inner.queue)).collect();
        v.sort_by_key(|v| std::cmp::Reverse(v.meta.created_ms));
        v
    }

    pub fn machine(&self) -> Machine {
        self.inner.locked().budget.machine()
    }

    /// The plan's usage limits as Claude Code last reported them, if it has.
    pub fn limits(&self) -> Option<Limits> {
        self.limits.locked().clone()
    }

    /// Tells every open window that the daemon is stopping on purpose.
    pub fn announce_stop(&self) {
        let _ = self.tx.send(UiEvent::Stopping);
    }

    /// Events after `after`, oldest first.
    pub fn events(&self, id: &str, after: u64) -> Result<Vec<Event>> {
        {
            let inner = self.inner.locked();
            let s = inner.sessions.get(id).context("no such session")?;
            if s.events.first().is_some_and(|first| first.seq <= after.saturating_add(1)) {
                return Ok(s.events.iter().filter(|e| e.seq > after).cloned().collect());
            }
        }
        Ok(self.store.events(id, after))
    }

    /// Folders to offer for a new session, most recently used first: where
    /// hangar sessions ran and where Claude Code was used (from its
    /// transcripts), then the rest of the folders Claude Code knows.
    pub fn recent_projects(&self) -> Vec<String> {
        const MAX: usize = 20;
        let claude_json = self.claude_json.locked().clone();
        let mut dated: Vec<(u64, String)> = {
            let inner = self.inner.locked();
            inner.sessions.values().map(|s| (s.meta.updated_ms, s.meta.cwd.clone())).collect()
        };
        dated.extend(recent::claude_folders(&recent::transcripts_dir(&claude_json)));
        dated.sort_by_key(|d| std::cmp::Reverse(d.0));
        let known = trust::claude_projects(&claude_json).into_iter().rev();
        recent::pick(dated.into_iter().map(|(_, c)| c).chain(known), MAX, &std::env::temp_dir())
    }

    /// Local MCP servers that Claude Code still starts on its own, for the
    /// app's "turn on sharing" notice. The Claude config is read again only
    /// when it has changed.
    pub fn unshared_servers(&self) -> Vec<crate::claude_config::Unshared> {
        let path = self.claude_json.locked().clone();
        let stamp: ConfigStamp = std::fs::metadata(&path).ok().map(|m| (m.modified().ok(), m.len()));
        let mut cache = self.unshared.locked();
        if let Some((at, found)) = cache.as_ref()
            && *at == stamp
        {
            return found.clone();
        }
        let found = crate::claude_config::unshared(&path);
        *cache = Some((stamp, found.clone()));
        found
    }

    /// What `hangar wrap` (`on`) and `hangar unwrap` do. Returns how many
    /// server entries changed.
    pub fn set_sharing(&self, on: bool) -> Result<usize> {
        let path = self.claude_json.locked().clone();
        Ok(crate::claude_config::set_shared(&path, &self.home, on, false)?.len())
    }

    /// Whether `cwd` is trusted, and what it would run.
    pub fn trust(&self, cwd: &Path) -> trust::TrustInfo {
        // The lock is not held while files a folder controls are read.
        let claude_json = self.claude_json.locked().clone();
        trust::inspect(&self.home, &claude_json, cwd)
    }

    // ------------------------------------------------------------ commands

    pub fn create(self: &Arc<Self>, req: NewSession) -> Result<SessionView> {
        let mut cwd = PathBuf::from(req.cwd.trim());
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("the first message is empty");
        }
        let mode = req.permission_mode.unwrap_or_else(|| "default".into());
        if !PERMISSION_MODES.contains(&mode.as_str()) {
            bail!("unknown permission mode {mode}");
        }
        let model = req.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
        if let Some(m) = &model
            && !valid_model(m)
        {
            bail!("{m:?} is not a model name");
        }
        // A new folder is made only now, when the session starts, so a
        // dialog that was closed leaves nothing behind. It is empty, so
        // there is nothing in it to distrust.
        // A relative path, or one with `..` in it, would mean a different
        // folder depending on where hangar was started, and trust is
        // remembered by path.
        if !cwd.is_absolute() || cwd.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            bail!("{} is not a full folder path", cwd.display());
        }
        if crate::folders::is_device_name(&cwd) {
            bail!("{} is a device name, not a folder", cwd.display());
        }
        let made = req.create_folder && !cwd.exists();
        if made {
            cwd = PathBuf::from(crate::folders::create(req.cwd.trim(), &crate::util::user_home())?);
        } else if !cwd.is_dir() {
            bail!("{} is not a folder", cwd.display());
        }
        let info = self.trust(&cwd);
        if !info.trusted {
            // A folder hangar has just made is empty. Unless a folder above
            // it brings MCP servers along, there is nothing to confirm.
            let nothing_to_confirm = made && info.lists_nothing();
            // Trust covers what the notice showed. When the folder sets
            // anything, the caller must name the notice it agreed to, and
            // that must still be the current one.
            let seen = match &req.trust_seen {
                Some(seen) => *seen == info.fingerprint,
                None => info.lists_nothing(),
            };
            if !nothing_to_confirm && !(req.trust && seen) {
                bail!(NeedsTrust(info));
            }
            trust::remember(&self.home, &cwd, &info.fingerprint);
        }
        let id = new_id();
        let now = now_ms();
        let title = req.title.filter(|t| !t.trim().is_empty()).map(|t| truncate(t.trim(), MAX_TITLE)).unwrap_or_else(|| title_from(&prompt));
        let meta = SessionMeta {
            id: id.clone(),
            title,
            cwd: cwd.display().to_string(),
            created_ms: now,
            updated_ms: now,
            model,
            permission_mode: mode,
            claude_session_id: None,
            status: Status::Queued,
            status_detail: None,
            cost_usd: 0.0,
            turns: 0,
            preview: String::new(),
            peak_bytes: 0,
            outbox: vec![prompt.clone()],
            outbox_ids: vec![crate::util::uuid()],
            context: None,
        };
        if let Err(e) = self.store.create(&id) {
            if made {
                let _ = std::fs::remove_dir(&cwd);
            }
            return Err(e.into());
        }
        {
            let mut inner = self.inner.locked();
            let uuid = meta.outbox_ids.first().cloned();
            let mode = Some(meta.permission_mode.clone());
            inner.sessions.insert(id.clone(), Session::new(meta, 1));
            self.push_event(&mut inner, &id, EventBody::User { text: prompt, uuid, mode });
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
        let mut inner = self.inner.locked();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let deliverable = s.stopping.is_none() && s.meta.status.is_live();
        let uuid = crate::util::uuid();
        match s.run.as_mut().filter(|_| deliverable) {
            Some(run) => {
                run.driver.send_user(&uuid, text);
                if !run.initialized {
                    run.unconfirmed.push((uuid.clone(), text.to_string()));
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
                s.meta.outbox_ids.resize_with(s.meta.outbox.len(), crate::util::uuid);
                s.meta.outbox.push(text.to_string());
                s.meta.outbox_ids.push(uuid.clone());
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
        let mode = inner.sessions.get(id).map(|s| s.meta.permission_mode.clone());
        self.push_event(&mut inner, id, EventBody::User { text: text.to_string(), uuid: Some(uuid), mode });
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        drop(inner);
        self.pump_queue();
        Ok(())
    }

    /// Answers a permission request. An answer that does not fit the request
    /// is refused and the request stays open.
    pub fn decide(&self, id: &str, request_id: &str, answer: Answer) -> Result<()> {
        let mut inner = self.inner.locked();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let run = s.run.as_ref().context("session is not running")?;
        let mut reply = reply_to(s.pending.get(request_id).context("that request is no longer pending")?, &answer)?;
        s.pending.remove(request_id);
        if reply.allow {
            // Claude Code's terminal hands Claude a note typed with an approval
            // together with the approval. The answer hangar sends has no place
            // for one, so the note goes in as a message just ahead of it:
            // Claude Code holds a message that arrives while a request waits,
            // and Claude reads it with the approval, before its next step.
            // (Sent after the approval, it is read one step too late. Both
            // orders were tried against Claude Code 2.1.325.)
            if let Some(note) = &reply.note {
                // To Claude Code the note is one more message of the user's.
                // Its id is kept, so that a later rewind can name it as seen.
                let uuid = crate::util::uuid();
                run.driver.send_user(&uuid, note);
                reply.detail.get_or_insert_with(|| json!({}))["note_id"] = json!(uuid);
            }
            run.driver.allow(request_id, &reply.input, reply.permissions.as_ref());
        } else {
            run.driver.deny(request_id, &reply.message, reply.interrupt);
        }
        // Claude Code confirms a mode change with a status message; the app
        // shows the new mode without waiting for it.
        if let Some(mode) = reply.mode {
            s.meta.permission_mode = mode;
        }
        let still_waiting = !s.pending.is_empty();
        self.push_event(&mut inner, id, EventBody::Decision { request_id: request_id.into(), behavior: reply.behavior.into(), detail: reply.detail });
        if !still_waiting {
            set_status(&mut inner, id, Status::Working, None);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        Ok(())
    }

    pub fn interrupt(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.locked();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let run = s.run.as_mut().context("session is not running")?;
        run.interrupted = Some(Instant::now());
        run.driver.interrupt();
        Ok(())
    }

    /// Sends Claude Code a request and returns where its answer will arrive.
    fn ask(&self, id: &str, request: Value) -> Result<oneshot::Receiver<Result<Value, String>>> {
        let mut inner = self.inner.locked();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        if s.stopping.is_some() {
            bail!("the session is stopping; try again in a moment");
        }
        let run = s.run.as_mut().context("session is not running")?;
        run.asked += 1;
        let request_id = format!("ask-{}", run.asked);
        let (tx, rx) = oneshot::channel();
        run.waiting.insert(request_id.clone(), tx);
        run.driver.request(&request_id, request);
        Ok(rx)
    }

    /// Claude Code's answer to `request`. An error is its refusal, in its words.
    async fn answer(&self, id: &str, request: Value) -> Result<Value> {
        let waiting = self.ask(id, request)?;
        match tokio::time::timeout(Duration::from_secs(60), waiting).await {
            Ok(Ok(Ok(answer))) => Ok(answer),
            Ok(Ok(Err(refusal))) => bail!("{refusal}"),
            Ok(Err(_)) => bail!("the session's process ended before it answered"),
            Err(_) => bail!("Claude Code did not answer in time"),
        }
    }

    /// What rewinding to the user's message `seq` needs: Claude Code's id
    /// for the message, the id of the newest message Claude has if that is
    /// a later one (Claude Code wants it named, to be sure that nothing
    /// newer is cut away unseen), and the mode the message was sent in.
    ///
    /// Read from the whole stored history: what is held in memory starts
    /// empty when the daemon starts, and is only the newest part of a long
    /// session.
    fn rewind_target(&self, id: &str, seq: u64) -> Result<RewindTarget> {
        // The user's messages that Claude still has, oldest first: (seq, id, a message of the feed, its mode).
        let mut kept: Vec<(u64, Option<String>, bool, Option<String>)> = Vec::new();
        for e in self.events(id, 0)? {
            match e.body {
                EventBody::User { uuid, mode, .. } => kept.push((e.seq, uuid, true, mode)),
                // A note typed with a plan's approval went to Claude as a message.
                EventBody::Decision { detail: Some(d), .. } if d["note_id"].is_string() => {
                    kept.push((e.seq, d["note_id"].as_str().map(String::from), false, None))
                }
                EventBody::Rewind { to, conversation: true, .. } => kept.retain(|(k, ..)| *k < to),
                _ => {}
            }
        }
        let newest = kept.last().and_then(|(_, u, ..)| u.clone());
        let Some((_, uuid, _, mode)) = kept.into_iter().find(|(k, _, message, _)| *k == seq && *message) else {
            bail!("that message is not in the conversation any more");
        };
        let uuid = uuid.context("this message was sent before hangar could rewind; only later ones can be gone back to")?;
        Ok(RewindTarget { newest: newest.filter(|n| *n != uuid), uuid, mode })
    }

    /// Makes sure the session has a process, starting one if it is stopped
    /// or paused. Rewinding is asked of Claude Code itself.
    fn wake(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.locked();
        let s = inner.sessions.get(id).context("no such session")?;
        if s.stopping.is_some() {
            bail!("the session is stopping; try again in a moment");
        }
        if s.run.is_some() {
            return Ok(());
        }
        inner.queue.retain(|q| q != id);
        self.launch(&mut inner, id);
        let s = inner.sessions.get(id).context("no such session")?;
        if s.run.is_none() {
            bail!("{}", s.meta.status_detail.clone().unwrap_or_else(|| "the session could not be started".into()));
        }
        Ok(())
    }

    /// Claude Code's rewind, for the user's message `seq`: a preview of what
    /// restoring the code would change, or the restore itself, of the code,
    /// of the conversation, or of both (the code first, as in Claude Code's
    /// terminal).
    ///
    /// The answer to a preview is `{can_restore, files, insertions,
    /// deletions, error}`. The answer to a restore has `code` and
    /// `conversation`, each `{ok, ...}`: for the code how many files were
    /// skipped, for the conversation the message's text to put back in the
    /// message box. A part that failed carries Claude Code's own `error`.
    pub async fn rewind(self: &Arc<Self>, id: &str, seq: u64, what: Rewind) -> Result<Value> {
        let RewindTarget { uuid, newest, mode } = self.rewind_target(id, seq)?;
        self.wake(id)?;
        if what == Rewind::Preview {
            let answer = self.answer(id, json!({"subtype": "rewind_files", "user_message_id": uuid, "dry_run": true})).await?;
            let preview = json!({
                "can_restore": answer["canRewind"] == true,
                "files": answer.get("filesChanged").cloned().unwrap_or_else(|| json!([])),
                "insertions": answer["insertions"].as_u64().unwrap_or(0),
                "deletions": answer["deletions"].as_u64().unwrap_or(0),
                "error": answer["error"],
            });
            if let Some(s) = self.inner.locked().sessions.get_mut(id) {
                s.preview = Some((seq, preview.clone()));
            }
            return Ok(preview);
        }
        // Claude Code's terminal stops a running turn before it restores
        // anything. The conversation cannot go back while a helper agent
        // still runs either (Claude Code refuses: "turn running"), so those
        // are stopped for it too. Stop ends the turn; with no turn left, it
        // ends the helpers.
        let with_helpers = matches!(what, Rewind::Conversation | Rewind::Both);
        let busy = || self.inner.locked().sessions.get(id).is_some_and(|s| s.run.as_ref().is_some_and(|r| r.turn || (with_helpers && !r.helpers.is_empty())));
        let give_up = Instant::now() + Duration::from_secs(20);
        let mut asked: Option<Instant> = None;
        while busy() {
            if asked.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                self.interrupt(id)?;
                asked = Some(Instant::now());
            }
            if Instant::now() > give_up {
                bail!("what is running did not stop");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut out = json!({});
        if matches!(what, Rewind::Code | Rewind::Both) {
            out["code"] = match self.answer(id, json!({"subtype": "rewind_files", "user_message_id": uuid})).await {
                Ok(answer) => json!({"ok": true, "skipped": answer["skippedLinks"].as_u64().unwrap_or(0)}),
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            };
        }
        // Backups that are still being copied (right after a resume) are the
        // one failure after which Claude Code's terminal leaves the
        // conversation alone too: a second try will do both.
        let try_again = out["code"]["error"].as_str().is_some_and(|e| e.contains("still being copied"));
        if matches!(what, Rewind::Conversation | Rewind::Both) && !try_again {
            let mut request = json!({"subtype": "rewind_conversation", "target_message_uuid": uuid});
            if let Some(newest) = &newest {
                request["last_seen_user_message_uuid"] = json!(newest);
            }
            out["conversation"] = match self.answer(id, request).await {
                Ok(answer) if answer["rewound"] == true => json!({"ok": true, "prefill": answer["prefillText"]}),
                Ok(answer) => {
                    json!({"ok": false, "error": answer["error"].as_str().unwrap_or("Claude Code did not rewind the conversation"), "reason": answer["reason"]})
                }
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            };
        }
        let (code, conversation) = (out["code"]["ok"] == true, out["conversation"]["ok"] == true);
        if code || conversation {
            let mut inner = self.inner.locked();
            if let Some(s) = inner.sessions.get_mut(id) {
                // The mode goes back to the one the message was sent in, as
                // in Claude Code's terminal. Driven like this, Claude Code
                // leaves the mode as it is (seen on 2.1.329), so hangar sets
                // it. Never to full access, though: that is only ever
                // switched on by the user saying so.
                if conversation
                    && let Some(mode) = mode.filter(|m| PERMISSION_MODES.contains(&m.as_str()) && m != "bypassPermissions" && *m != s.meta.permission_mode)
                {
                    if let Some(run) = &s.run {
                        run.driver.set_permission_mode(&mode);
                    }
                    out["conversation"]["mode"] = json!(mode);
                    s.meta.permission_mode = mode;
                }
                // What the files got back is what the preview of this message last said.
                let restored = code.then(|| {
                    let mut restored = s
                        .preview
                        .take()
                        .filter(|(of, _)| *of == seq)
                        .map(|(_, p)| json!({"files": p["files"], "insertions": p["insertions"], "deletions": p["deletions"]}))
                        .unwrap_or_else(|| json!({}));
                    restored["skipped"] = out["code"]["skipped"].clone();
                    restored
                });
                if let Some(run) = s.run.as_mut() {
                    run.ask_context();
                }
                self.push_event(&mut inner, id, EventBody::Rewind { to: seq, code: restored, conversation });
                self.save(&inner, id);
                self.broadcast_session(&inner, id);
            }
        }
        Ok(out)
    }

    /// Stops one helper agent of a session. What else is running carries on.
    pub fn stop_helper(&self, id: &str, helper: &str) -> Result<()> {
        let mut inner = self.inner.locked();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        let run = s.run.as_mut().context("session is not running")?;
        if !run.helpers.iter().any(|h| h.id == helper) {
            bail!("that helper is not running");
        }
        run.stopped_alone.insert(helper.to_string());
        run.driver.stop_task(helper);
        Ok(())
    }

    /// Stops the process and drops undelivered messages. The conversation can be resumed later.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.locked();
        inner.queue.retain(|q| q != id);
        let s = inner.sessions.get_mut(id).context("no such session")?;
        s.meta.outbox.clear();
        s.meta.outbox_ids.clear();
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
        let mut inner = self.inner.locked();
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

    /// Renames a session or changes its permission mode or model. A running
    /// session switches at once (from its next turn); a stopped or paused
    /// one resumes with the new settings.
    pub fn update(&self, id: &str, change: Update) -> Result<SessionView> {
        let mut inner = self.inner.locked();
        let s = inner.sessions.get_mut(id).context("no such session")?;
        if let Some(m) = &change.permission_mode
            && !PERMISSION_MODES.contains(&m.as_str())
        {
            bail!("unknown permission mode {m}");
        }
        // `Some("")` asks for Claude Code's default model.
        let model = change.model.map(|m| Some(m.trim().to_string()).filter(|m| !m.is_empty()));
        if let Some(Some(m)) = &model
            && !valid_model(m)
        {
            bail!("{m:?} is not a model name");
        }
        if let Some(t) = change.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            s.meta.title = truncate(&t, MAX_TITLE);
        }
        if let Some(m) = change.permission_mode {
            if let Some(run) = &s.run {
                run.driver.set_permission_mode(&m);
            }
            s.meta.permission_mode = m;
        }
        if let Some(m) = model {
            if let Some(run) = &s.run {
                run.driver.set_model(m.as_deref());
            }
            s.meta.model = m;
        }
        s.meta.updated_ms = now_ms();
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
        Ok(view(inner.sessions.get(id).unwrap(), &inner.queue))
    }

    pub fn remove(self: &Arc<Self>, id: &str) -> Result<()> {
        let mut inner = self.inner.locked();
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
        let mut inner = self.inner.locked();
        for s in inner.sessions.values_mut() {
            if let Some(run) = &s.run {
                s.stopping = Some(if s.meta.claude_session_id.is_some() { Status::Hibernated } else { Status::Stopped });
                run.driver.kill();
            }
        }
    }

    // ------------------------------------------------------------ budget

    fn view_of(&self, id: &str) -> Option<SessionView> {
        let inner = self.inner.locked();
        inner.sessions.get(id).map(|s| view(s, &inner.queue))
    }

    /// Starts queued sessions while the budget allows, oldest first.
    pub fn pump_queue(self: &Arc<Self>) {
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return; // the daemon is going away; queued sessions start next time
        }
        let mut inner = self.inner.locked();
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
        let claude = self.claude_bin.locked().clone().or_else(crate::agent::find_claude);
        let Some(s) = inner.sessions.get_mut(id) else { return };
        let Some(claude) = claude else {
            s.meta.status = Status::Error;
            s.meta.status_detail = Some(crate::agent::NOT_FOUND.into());
            self.save(inner, id);
            self.broadcast_session(inner, id);
            return;
        };
        // The folder's settings may no longer be the ones the user agreed
        // to (a pull, a branch switch, a file Claude wrote), and a new
        // process would act on them unseen.
        let folder = PathBuf::from(&s.meta.cwd);
        if folder.is_dir() && !self.trust(&folder).trusted {
            s.meta.status = Status::Error;
            s.meta.status_detail = Some(CONFIRM_FOLDER.into());
            self.save(inner, id);
            self.broadcast_session(inner, id);
            return;
        }
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
                let texts = std::mem::take(&mut s.meta.outbox);
                let mut ids = std::mem::take(&mut s.meta.outbox_ids);
                ids.resize_with(texts.len(), crate::util::uuid);
                let outbox: Vec<(String, String)> = ids.into_iter().zip(texts).collect();
                for (uuid, text) in &outbox {
                    driver.send_user(uuid, text);
                }
                s.run = Some(Run {
                    driver,
                    initialized: false,
                    unconfirmed: outbox,
                    waiting: HashMap::new(),
                    asked: 0,
                    cost_seen: 0.0,
                    completed_turns: 0,
                    resumed: resume.is_some(),
                    context_asked: None,
                    context_stale: false,
                    helpers: Vec::new(),
                    helper_ids: BTreeSet::new(),
                    turn: false,
                    settle: None,
                    interrupted: None,
                    stopped_alone: BTreeSet::new(),
                    report_due: false,
                });
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
                    let mut inner = self.inner.locked();
                    if let Some(s) = inner.sessions.get_mut(&session_id).filter(|s| s.pid() == Some(pid)) {
                        s.mcp = servers;
                        self.broadcast_session(&inner, &session_id);
                    }
                }
                E::Context { session_id, pid, usage } => self.on_context(&session_id, pid, &usage),
                E::Answer { session_id, pid, request_id, answer } => self.on_answer(&session_id, pid, &request_id, answer),
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
        let mut inner = self.inner.locked();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let run = s.run.as_mut().expect("current session has a run");
        let kind = msg["type"].as_str().unwrap_or_default().to_string();
        let subtype = msg["subtype"].as_str().unwrap_or_default().to_string();
        let mut status = None;
        match (kind.as_str(), subtype.as_str()) {
            ("system", "init") => {
                run.initialized = true;
                run.unconfirmed.clear();
                // Both become command-line arguments on the next start, so
                // they are held to the same rules as what the user types.
                if let Some(sid) = msg["session_id"].as_str().filter(|s| valid_conversation(s)) {
                    s.meta.claude_session_id = Some(sid.to_string());
                }
                if s.meta.model.is_none() {
                    s.meta.model = msg["model"].as_str().filter(|m| valid_model(m)).map(String::from);
                }
                // Modes can change inside Claude (plan mode exits, for one).
                if let Some(mode) = msg["permissionMode"].as_str().filter(|m| PERMISSION_MODES.contains(m)) {
                    s.meta.permission_mode = mode.to_string();
                }
                if let Some(list) = msg["mcp_servers"].as_array() {
                    s.mcp = list.iter().map(|m| json!({"name": m["name"], "status": m["status"]})).collect();
                }
                // Every turn opens with `init`, also one Claude Code starts by
                // itself when a helper has reported back.
                run.turn = true;
                run.settle = None;
                run.report_due = false;
                if matches!(s.meta.status, Status::Starting | Status::Idle) {
                    status = Some(Status::Working);
                }
                run.ask_context();
            }
            ("system", "task_started" | "task_updated" | "task_notification" | "background_tasks_changed") => {
                let alone = msg["task_id"].as_str().is_some_and(|t| run.stopped_alone.remove(t));
                if let Some(ended) = run.track(&subtype, &msg) {
                    // Stop ended this helper with everything else: nothing follows.
                    let with_the_rest = ended.stopped && !alone && run.just_interrupted();
                    if run.at_rest() {
                        if with_the_rest {
                            if s.meta.status == Status::Working {
                                s.idle_since = Some(Instant::now());
                                status = Some(Status::Idle);
                            }
                        } else {
                            // Claude Code now starts a turn in which Claude reads the
                            // report, or hears that the helper was stopped by itself.
                            run.settle = Some(Instant::now());
                        }
                    } else if run.turn && ended.background && !with_the_rest {
                        // The same turn, once the one that is running has ended.
                        run.report_due = true;
                    }
                }
            }
            // Claude Code says so when the mode changes inside it: a plan
            // was approved, or it left plan mode some other way.
            ("system", "status") => {
                if let Some(mode) = msg["permissionMode"].as_str().filter(|m| PERMISSION_MODES.contains(m)) {
                    s.meta.permission_mode = mode.to_string();
                }
            }
            ("system", "compact_boundary") => run.ask_context(),
            ("assistant", _) => {
                // A subagent's messages are about its own context, not this one.
                let own = msg["parent_tool_use_id"].is_null();
                if own && let Some(text) = first_text(&msg) {
                    s.meta.preview = truncate(&text, 160);
                }
                // A late message from a helper that has ended does not mean that anything runs.
                let at_work = own || run.helpers.iter().any(|h| msg["parent_tool_use_id"] == h.tool_use_id.as_str());
                if at_work && matches!(s.meta.status, Status::Starting | Status::Idle) {
                    status = Some(Status::Working);
                }
                if own {
                    run.ask_context();
                }
            }
            ("rate_limit_event", _) => {
                if let Some(limits) = Limits::read(&msg["rate_limit_info"], now_ms()) {
                    self.set_limits(limits);
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
                if let Some(sid) = msg["session_id"].as_str().filter(|s| valid_conversation(s)) {
                    s.meta.claude_session_id = Some(sid.to_string());
                }
                if msg["is_error"].as_bool().unwrap_or(false) {
                    // A turn the user stopped did not fail.
                    let stopped = msg["terminal_reason"].as_str().is_some_and(|r| r.starts_with("aborted"));
                    s.meta.preview = truncate(msg["result"].as_str().unwrap_or(if stopped { "Stopped" } else { "the turn failed" }), 160);
                }
                run.turn = false;
                // A helper in the foreground runs inside the turn: it cannot
                // have outlived it, whatever was or was not said about its end.
                run.helpers.retain(|h| h.background || h.depth > 1);
                if !run.helpers.is_empty() {
                    // A helper in the background works on after Claude's turn has
                    // ended: the session is not done, and must not be paused.
                    status = Some(Status::Working);
                } else if std::mem::take(&mut run.report_due) {
                    // A helper reported while this turn ran: the turn about it comes next.
                    run.settle = Some(Instant::now());
                    status = Some(Status::Working);
                } else {
                    s.idle_since = Some(Instant::now());
                    status = Some(Status::Idle);
                }
                run.ask_context();
            }
            _ => {}
        }
        let keep = match kind.as_str() {
            // `command_lifecycle` reports each message with an id as queued, started and completed.
            "stream_event" | "rate_limit_event" | "command_lifecycle" => false,
            "system" => match subtype.as_str() {
                "init" | "compact_boundary" => true,
                // Of the background tasks, only helper agents are drawn.
                "task_started" | "task_progress" | "task_updated" | "task_notification" => msg["task_id"].as_str().is_some_and(|t| run.helper_ids.contains(t)),
                _ => false,
            },
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

    /// Claude Code answered a request someone waits for.
    fn on_answer(&self, id: &str, pid: u32, request_id: &str, answer: Result<Value, String>) {
        let mut inner = self.inner.locked();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let run = s.run.as_mut().expect("current session has a run");
        if let Some(waiter) = run.waiting.remove(request_id) {
            let _ = waiter.send(answer);
        }
        // A process started only to rewind is sent no message, so it never
        // reports `init`. That it answers shows that it is up.
        if !run.initialized && run.unconfirmed.is_empty() && s.meta.status == Status::Starting {
            run.initialized = true;
            s.idle_since = Some(Instant::now());
            set_status(&mut inner, id, Status::Idle, None);
            self.save(&inner, id);
            self.broadcast_session(&inner, id);
        }
    }

    fn on_context(&self, id: &str, pid: u32, usage: &Value) {
        let mut inner = self.inner.locked();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let run = s.run.as_mut().expect("current session has a run");
        run.context_asked = None;
        if run.context_stale {
            run.ask_context();
        }
        let Some(mut now) = ContextUsage::read(usage, now_ms()) else { return };
        // The same numbers as before change nothing worth saving or sending.
        if let Some(before) = &s.meta.context {
            let at = now.at_ms;
            now.at_ms = before.at_ms;
            if *before == now {
                return;
            }
            now.at_ms = at;
        }
        s.meta.context = Some(now);
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    /// Keeps the newest limits, in memory and on disk (so they show before
    /// the first session of the day has run).
    fn set_limits(&self, limits: Limits) {
        let mut held = self.limits.locked();
        if held.as_ref().is_some_and(|h| h.windows == limits.windows) {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec(&limits) {
            let _ = crate::util::write_private(&self.home.join(LIMITS_FILE), &bytes);
        }
        *held = Some(limits);
    }

    fn on_permission(&self, id: &str, pid: u32, req: claude::PermissionRequest) {
        let mut inner = self.inner.locked();
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
                agent_id: req.agent_id.clone(),
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
                agent_id: req.agent_id,
            },
        );
        set_status(&mut inner, id, Status::NeedsYou, None);
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_cancelled(&self, id: &str, pid: u32, request_id: &str) {
        let mut inner = self.inner.locked();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        if s.pending.remove(request_id).is_none() {
            return;
        }
        let still_waiting = !s.pending.is_empty();
        let needs_you = s.meta.status == Status::NeedsYou;
        // Stop ends a helper that was asking with everything else: nothing is
        // left running, and nothing follows. (A helper stopped by itself is
        // followed by a turn; see `SETTLE`.)
        let done = s.run.as_ref().is_some_and(|r| r.at_rest() && r.just_interrupted());
        if done && !still_waiting && needs_you {
            s.idle_since = Some(Instant::now());
        }
        self.push_event(&mut inner, id, EventBody::Decision { request_id: request_id.into(), behavior: "cancelled".into(), detail: None });
        if !still_waiting && needs_you {
            set_status(&mut inner, id, if done { Status::Idle } else { Status::Working }, None);
        }
        self.save(&inner, id);
        self.broadcast_session(&inner, id);
    }

    fn on_exit(&self, id: &str, pid: u32, code: Option<i32>, stderr: Vec<String>) {
        let mut inner = self.inner.locked();
        let Some(s) = Self::current(&mut inner, id, pid) else { return };
        let run = s.run.take().expect("current session has a run");
        // Requests the process was waiting on are over: say so in the
        // history, so none of them can look answerable later.
        let withdrawn: Vec<String> = std::mem::take(&mut s.pending).into_keys().collect();
        s.memory = None;
        let peak = s.meta.peak_bytes;
        if !run.initialized && !run.unconfirmed.is_empty() {
            // Claude never started reading: those messages are still unsent.
            let (mut ids, mut texts): (Vec<String>, Vec<String>) = run.unconfirmed.into_iter().unzip();
            s.meta.outbox_ids.resize_with(s.meta.outbox.len(), crate::util::uuid);
            ids.append(&mut s.meta.outbox_ids);
            texts.append(&mut s.meta.outbox);
            s.meta.outbox = texts;
            s.meta.outbox_ids = ids;
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
        for request_id in withdrawn {
            self.push_event(&mut inner, id, EventBody::Decision { request_id, behavior: "cancelled".into(), detail: None });
        }
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
                let mut inner = self.inner.locked();
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
                    // The last helper ended and Claude started no turn to read its report.
                    let settled = s.run.as_ref().is_some_and(|r| r.at_rest() && r.settle.is_some_and(|t| t.elapsed() >= SETTLE));
                    if settled {
                        if let Some(run) = s.run.as_mut() {
                            run.settle = None;
                        }
                        if s.meta.status == Status::Working {
                            s.idle_since = Some(Instant::now());
                            set_status(&mut inner, &id, Status::Idle, None);
                            self.save(&inner, &id);
                            self.broadcast_session(&inner, &id);
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
        let _ = self.tx.send(UiEvent::Event { session_id: id.to_string(), event: Box::new(ev) });
    }

    fn save(&self, inner: &Inner, id: &str) {
        if let Some(s) = inner.sessions.get(id) {
            self.store.save_meta(&s.meta);
        }
    }

    fn broadcast_session(&self, inner: &Inner, id: &str) {
        if let Some(s) = inner.sessions.get(id) {
            let _ = self.tx.send(UiEvent::Session { session: Box::new(view(s, &inner.queue)) });
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
        agents: s.run.as_ref().map(|r| r.helpers.clone()).unwrap_or_default(),
        in_turn: s.run.as_ref().is_some_and(|r| r.turn),
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
    // Of a task message, only what a helper's row shows. The prompt is in the
    // Agent call already, and the report arrives as the helper's own message.
    if msg["type"] == "system" && msg["subtype"].as_str().is_some_and(|s| s.starts_with("task_")) {
        let mut kept = json!({"type": "system", "subtype": msg["subtype"], "task_id": msg["task_id"]});
        for key in ["tool_use_id", "description", "subagent_type", "is_backgrounded", "status", "usage", "last_tool_name"] {
            if let Some(v) = msg.get(key).filter(|v| !v.is_null()) {
                kept[key] = match v {
                    Value::String(s) => json!(truncate(s, 200)),
                    v => v.clone(),
                };
            }
        }
        if let Some(status) = msg["patch"]["status"].as_str() {
            kept["status"] = json!(status);
        }
        return kept;
    }
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
        // Claude Code repeats each tool's output here in full (a whole file,
        // before and after). Only the line-by-line changes of an edit are kept.
        if let Some(o) = msg.as_object_mut()
            && let Some(kept) = o.remove("tool_use_result").as_ref().and_then(|r| patch_of(r).or_else(|| helper_totals(r)))
        {
            o.insert("tool_use_result".into(), kept);
        }
    }
    msg
}

/// What the Agent tool reports about a helper: whether it went to the
/// background, and for one that ran to its end, how much it did.
fn helper_totals(result: &Value) -> Option<Value> {
    let id = result["agentId"].as_str().filter(|a| a.len() <= 64)?;
    let mut kept = json!({"agentId": id});
    for key in ["status", "totalDurationMs", "totalTokens", "totalToolUseCount"] {
        if let Some(v) = result.get(key).filter(|v| v.is_number() || v.as_str().is_some_and(|s| s.len() <= 32)) {
            kept[key] = v.clone();
        }
    }
    Some(kept)
}

/// The most lines of one edit's changes kept, and the longest line.
const MAX_PATCH_LINES: usize = 400;
const MAX_PATCH_LINE: usize = 1000;

/// The changes an Edit or Write made, as Claude Code reports them: for each
/// changed stretch, the line it starts at before and after, and its lines
/// (` ` unchanged, `-` removed, `+` added). `None` for any other tool.
fn patch_of(result: &Value) -> Option<Value> {
    let hunks = result["structuredPatch"].as_array()?;
    let mut left = MAX_PATCH_LINES;
    let mut truncated = false;
    let mut kept = Vec::new();
    for h in hunks {
        let (Some(old_start), Some(new_start), Some(lines)) = (h["oldStart"].as_u64(), h["newStart"].as_u64(), h["lines"].as_array()) else { continue };
        if left == 0 {
            truncated = true;
            break;
        }
        let mut out = Vec::new();
        for line in lines.iter().filter_map(Value::as_str) {
            if left == 0 {
                truncated = true;
                break;
            }
            left -= 1;
            let mut end = line.len().min(MAX_PATCH_LINE);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            out.push(if end < line.len() { format!("{}…", &line[..end]) } else { line.to_string() });
        }
        kept.push(json!({"oldStart": old_start, "newStart": new_start, "lines": out}));
    }
    let mut v = json!({"structuredPatch": kept});
    // Write says whether the file is new.
    if let Some(kind) = result["type"].as_str().filter(|k| ["create", "update"].contains(k)) {
        v["type"] = json!(kind);
    }
    if truncated {
        v["truncated"] = json!(true);
    }
    Some(v)
}

/// Where the newest plan limits are kept between runs.
const LIMITS_FILE: &str = "limits.json";

pub fn title_from(prompt: &str) -> String {
    let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("New session").trim();
    truncate(first, 60)
}

fn new_id() -> String {
    format!("s{}", crate::util::random_hex(6))
}

/// The longest title kept; longer ones are cut.
const MAX_TITLE: usize = 200;

/// Claude's conversation ids become a `--resume=` argument: letters, digits,
/// `-` and `_` only.
fn valid_conversation(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Model names become a `claude --model` argument: letters, digits and
/// `.-_:[]/` only, never starting with `-` (which would read as a flag).
fn valid_model(m: &str) -> bool {
    !m.is_empty() && m.len() <= 100 && !m.starts_with('-') && m.chars().all(|c| c.is_ascii_alphanumeric() || ".-_:[]/".contains(c))
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
    fn model_names_cannot_smuggle_flags() {
        for ok in ["haiku", "opus", "claude-sonnet-5-5", "claude-opus-5-5[1m]", "us.anthropic.claude-haiku-5-5"] {
            assert!(valid_model(ok), "{ok}");
        }
        for bad in ["--dangerously-skip-permissions", "-p", "haiku --verbose", "a;b", "x\ny", ""] {
            assert!(!valid_model(bad), "{bad:?}");
        }
    }

    #[test]
    fn ids_are_validated_before_touching_the_disk() {
        assert!(valid_id(&new_id()));
        assert!(!valid_id("../../etc"));
        assert!(!valid_id("s12345678901z"));
        assert!(!valid_id("x123456789012"));
    }

    fn pending(tool: &str, input: Value) -> PendingPermission {
        let suggestions =
            json!([{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "ls"}], "behavior": "allow", "destination": "localSettings"}]);
        PendingPermission { request_id: "r".into(), tool_name: tool.into(), input, suggestions: Some(suggestions), can_remember: true, agent_id: None }
    }

    fn answers(pairs: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
        Some(pairs.iter().map(|(q, a)| (q.to_string(), a.to_string())).collect())
    }

    #[test]
    fn a_question_goes_back_with_the_answers_and_nothing_else_can_carry_them() {
        let ask = pending(ASK_TOOL, json!({"questions": [{"question": "Which colour?"}, {"question": "Which fruit?"}]}));
        let yes = Answer { allow: true, ..Default::default() };
        let r = reply_to(&ask, &Answer { answers: answers(&[("Which colour?", " Green "), ("Which fruit?", "Apple, Cherry")]), always: true, ..yes.clone() })
            .unwrap();
        assert_eq!(r.input["answers"], json!({"Which colour?": "Green", "Which fruit?": "Apple, Cherry"}));
        assert_eq!(r.input["questions"], ask.input["questions"], "the questions go back as they came");
        assert_eq!((r.behavior, &r.permissions), ("allow", &None), "there are no rules to remember for a question");
        assert_eq!(r.detail, Some(json!({"answers": {"Which colour?": "Green", "Which fruit?": "Apple, Cherry"}})));
        // One answered of two is still an answer; none is not.
        assert!(reply_to(&ask, &Answer { answers: answers(&[("Which colour?", "Blue"), ("Which fruit?", "  ")]), ..yes.clone() }).is_ok());
        assert!(reply_to(&ask, &yes).is_err());
        assert!(reply_to(&ask, &Answer { answers: answers(&[("Which colour?", " ")]), ..yes.clone() }).is_err());
        assert!(reply_to(&ask, &Answer { answers: answers(&[("Delete everything?", "Yes")]), ..yes.clone() }).is_err(), "not a question Claude asked");
        // Answers replace a tool's input, so no other tool takes them.
        let bash = pending("Bash", json!({"command": "ls"}));
        assert!(reply_to(&bash, &Answer { answers: answers(&[("command", "rm -rf /")]), ..yes.clone() }).is_err());
        assert!(reply_to(&ask, &Answer { answers: answers(&[("Which colour?", "Red")]), ..Default::default() }).is_err(), "answers do not go with a refusal");
        let r = reply_to(&bash, &Answer { always: true, ..yes.clone() }).unwrap();
        assert_eq!((r.input.clone(), r.behavior), (json!({"command": "ls"}), "allow_always"));
        assert_eq!(r.permissions.unwrap()[0]["destination"], "session");

        let r = reply_to(&ask, &Answer { clarify: true, ..Default::default() }).unwrap();
        assert!(
            !r.allow
                && !r.interrupt
                && r.message.starts_with("The user wants to clarify these questions.")
                && r.message.contains("- Which fruit?\n  (No answer provided)"),
            "{}",
            r.message
        );
        assert!(reply_to(&bash, &Answer { clarify: true, ..Default::default() }).is_err());
        let long = "x".repeat(MAX_REPLY * 2);
        let r = reply_to(&ask, &Answer { answers: answers(&[("Which colour?", &long)]), ..yes }).unwrap();
        assert_eq!(r.input["answers"]["Which colour?"].as_str().unwrap().len(), MAX_REPLY);
    }

    #[test]
    fn a_plan_continues_in_the_chosen_mode_or_goes_back_in_claudes_own_words() {
        let plan = pending(PLAN_TOOL, json!({"plan": "# Plan"}));
        let yes = Answer { allow: true, ..Default::default() };
        let r = reply_to(&plan, &Answer { mode: Some("acceptEdits".into()), ..yes.clone() }).unwrap();
        assert_eq!(r.permissions, Some(json!([{"type": "setMode", "mode": "acceptEdits", "destination": "session"}])));
        assert_eq!((r.mode.as_deref(), r.input.clone()), (Some("acceptEdits"), json!({"plan": "# Plan"})));
        for bad in ["bypassPermissions", "plan", "--dangerously-skip-permissions", ""] {
            assert!(reply_to(&plan, &Answer { mode: Some(bad.into()), ..yes.clone() }).is_err(), "{bad:?}");
        }
        assert!(
            reply_to(&pending("Bash", json!({"command": "ls"})), &Answer { mode: Some("acceptEdits".into()), ..yes.clone() }).is_err(),
            "only a plan changes the mode"
        );
        let r = reply_to(&plan, &Answer { always: true, ..yes.clone() }).unwrap();
        assert_eq!((r.behavior, r.permissions, r.mode, r.note), ("allow", None, None, None), "a plan never takes Claude's suggested rules");

        // Approved with a note: the note goes to Claude as it was typed, and is kept in the history.
        let r = reply_to(&plan, &Answer { mode: Some("default".into()), feedback: Some("  skip step 4 ".into()), ..yes.clone() }).unwrap();
        assert_eq!((r.allow, r.note.as_deref(), r.mode.as_deref()), (true, Some("skip step 4"), Some("default")));
        assert_eq!(r.detail, Some(json!({"mode": "default", "feedback": "skip step 4"})));
        let r = reply_to(&plan, &Answer { mode: Some("default".into()), feedback: Some("   ".into()), ..yes.clone() }).unwrap();
        assert_eq!((r.note, r.detail), (None, Some(json!({"mode": "default"}))), "a note of spaces is no note");
        assert!(
            reply_to(&pending("Bash", json!({"command": "ls"})), &Answer { feedback: Some("and then delete it".into()), ..yes }).is_err(),
            "only a plan is approved with a note"
        );

        let r = reply_to(&plan, &Answer { feedback: Some("  add tests first ".into()), stop: true, ..Default::default() }).unwrap();
        assert_eq!(r.message, format!("{REFUSED}{REFUSED_BECAUSE}add tests first"));
        assert!(!r.interrupt, "with something to act on, the turn goes on");
        let r = reply_to(&plan, &Answer { stop: true, ..Default::default() }).unwrap();
        assert!(r.interrupt && r.message == format!("{REFUSED}{REFUSED_STOP}"));
        assert_eq!(r.detail, Some(json!({"stopped": true})));
        let r = reply_to(&plan, &Answer { feedback: Some("   ".into()), ..Default::default() }).unwrap();
        assert_eq!((r.message.as_str(), r.interrupt, r.detail), ("The user declined this tool use.", false, None));
    }

    #[test]
    fn context_and_limits_are_claudes_numbers_or_nothing() {
        let v = json!({"totalTokens": 28884, "maxTokens": 1000000, "percentage": 3, "model": "claude-haiku-5-5", "autoCompactThreshold": 967000, "isAutoCompactEnabled": true, "categories": [
            {"name": "System prompt", "tokens": 3432, "kind": "used"},
            {"name": "System tools (deferred)", "tokens": 18433, "kind": "deferred"},
            {"name": "Messages\u{202e}", "tokens": 7749, "kind": "used"},
            {"name": "Nothing yet", "tokens": 0, "kind": "used"},
            {"name": "Free space", "tokens": 938116, "kind": "free"}
        ]});
        let c = ContextUsage::read(&v, 7).unwrap();
        assert_eq!((c.used_tokens, c.max_tokens, c.percent, c.auto_compact_at, c.at_ms), (28884, 1000000, 3, Some(967000), 7));
        let parts: Vec<(&str, u64)> = c.parts.iter().map(|p| (p.name.as_str(), p.tokens)).collect();
        assert_eq!(parts, [("System prompt", 3432), ("Messages", 7749)], "only what is in use, with invisible characters gone");
        let mut odd = v.clone();
        odd["isAutoCompactEnabled"] = json!(false);
        odd["percentage"] = json!(250.4);
        let c = ContextUsage::read(&odd, 7).unwrap();
        assert_eq!((c.percent, c.auto_compact_at), (100, None));
        for missing in ["percentage", "totalTokens", "maxTokens"] {
            let mut without = v.clone();
            without.as_object_mut().unwrap().remove(missing);
            assert_eq!(ContextUsage::read(&without, 7), None, "without Claude's own {missing} there is nothing to show");
        }
        assert_eq!(ContextUsage::read(&Value::Null, 7), None);

        let info = json!({"status": "allowed", "resetsAt": 1794791400u64, "rateLimitType": "five_hour", "unifiedWindows": {
            "seven_day": {"utilization": 0.1, "resetsAt": 1795197600u64, "status": "allowed"},
            "five_hour": {"utilization": 0.46, "resetsAt": 1794791400u64, "status": "allowed_warning"},
            "Not A Name": {"utilization": 0.5},
            "seven_day_opus": {"utilization": "a lot"}
        }});
        let l = Limits::read(&info, 9).unwrap();
        let seen: Vec<(&str, f64, Option<u64>, &str)> = l.windows.iter().map(|w| (w.name.as_str(), w.used, w.resets_at, w.status.as_str())).collect();
        assert_eq!(seen, [("five_hour", 0.46, Some(1794791400), "allowed_warning"), ("seven_day", 0.1, Some(1795197600), "allowed")]);
        // An older shape names one window; a time that is not one is dropped, a status that is not one reads as allowed.
        let one = Limits::read(&json!({"status": "weird", "rateLimitType": "seven_day", "utilization": 1.0, "resetsAt": 5}), 9).unwrap();
        assert_eq!(one.windows, [LimitWindow { name: "seven_day".into(), used: 1.0, resets_at: None, status: "allowed".into() }]);
        assert_eq!(Limits::read(&json!({"status": "allowed", "rateLimitType": "five_hour"}), 9), None, "no usage figure: nothing to show");
    }

    #[test]
    fn every_kind_of_event_can_be_read_back_from_the_history() {
        // An event is stored as one object: its own number and time beside the fields of what
        // happened. A field of that name inside would be written twice and not read back.
        let bodies = vec![
            EventBody::User { text: "hi".into(), uuid: Some("u".into()), mode: Some("default".into()) },
            EventBody::Claude { msg: json!({"type": "result"}) },
            EventBody::Permission {
                request_id: "r".into(),
                tool_name: "Bash".into(),
                input: json!({}),
                suggestions: None,
                description: None,
                agent_id: Some("a".into()),
            },
            EventBody::Decision { request_id: "r".into(), behavior: "allow".into(), detail: Some(json!({"mode": "default"})) },
            EventBody::Rewind { to: 3, code: Some(json!({"files": ["a.txt"]})), conversation: true },
            EventBody::Status { status: Status::Idle, detail: None },
            EventBody::Error { message: "x".into() },
        ];
        for body in bodies {
            let line = serde_json::to_string(&Event { seq: 7, ts: 9, body: body.clone() }).unwrap();
            let back: Event = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line}: {e}"));
            assert_eq!((back.seq, back.ts), (7, 9), "{line}");
            assert_eq!(serde_json::to_value(&back.body).unwrap(), serde_json::to_value(&body).unwrap(), "{line}");
        }
    }

    #[test]
    fn slim_keeps_only_the_changed_lines_of_an_edit() {
        let edit = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}, "tool_use_result": {
            "filePath": "C:/repo/a.ts", "originalFile": "x".repeat(50_000), "oldString": "a", "newString": "b", "userModified": false,
            "structuredPatch": [{"oldStart": 14, "oldLines": 3, "newStart": 14, "newLines": 3, "lines": [" line 16", "-line 17", "+LINE"]}]
        }});
        assert_eq!(slim(edit)["tool_use_result"], json!({"structuredPatch": [{"oldStart": 14, "newStart": 14, "lines": [" line 16", "-line 17", "+LINE"]}]}));
        let created = json!({"type": "user", "message": {"content": []}, "tool_use_result": {"type": "create", "content": "the whole file", "structuredPatch": [], "originalFile": null}});
        assert_eq!(slim(created)["tool_use_result"], json!({"structuredPatch": [], "type": "create"}));
        // A huge change is cut at a whole line and says so; a huge line is cut at a whole character.
        let many: Vec<String> = (0..MAX_PATCH_LINES + 50).map(|i| format!("+{i}")).collect();
        let big = json!({"type": "user", "message": {"content": []}, "tool_use_result": {"structuredPatch": [
            {"oldStart": 1, "newStart": 1, "lines": many}, {"oldStart": 900, "newStart": 950, "lines": ["+never reached"]}
        ]}});
        let kept = slim(big)["tool_use_result"].clone();
        assert_eq!(kept["structuredPatch"].as_array().unwrap().len(), 1);
        assert_eq!(kept["structuredPatch"][0]["lines"].as_array().unwrap().len(), MAX_PATCH_LINES);
        assert_eq!(kept["truncated"], true);
        let wide = json!({"type": "user", "message": {"content": []}, "tool_use_result": {"structuredPatch": [{"oldStart": 1, "newStart": 1, "lines": [format!("+{}", "\u{e9}".repeat(MAX_PATCH_LINE))]}]}});
        let line = slim(wide)["tool_use_result"]["structuredPatch"][0]["lines"][0].as_str().unwrap().to_string();
        assert!(line.len() <= MAX_PATCH_LINE + 3 && line.ends_with('\u{2026}'), "{}", line.len());
    }

    #[test]
    fn status_serializes_snake_case() {
        assert_eq!(serde_json::to_value(Status::NeedsYou).unwrap(), "needs_you");
    }
}
