//! Session manager and UI API tests, driven by a fake `claude`.

mod common;

use common::*;
use hangar::config::Config;
use hangar::daemon::{Daemon, DaemonOptions};
use hangar::sessions::{EventBody, Manager, NeedsTrust, NewSession, SessionView, Status};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn fake_claude() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_fake-claude").into()
}

fn manager(home: &std::path::Path, config: Config) -> Arc<Manager> {
    let m = Manager::start(home, &config);
    m.set_claude_bin(Some(fake_claude()));
    m.set_claude_json(home.join("no-claude.json"));
    m
}

fn new(cwd: &std::path::Path, prompt: &str) -> NewSession {
    NewSession { cwd: cwd.display().to_string(), prompt: prompt.into(), title: None, model: None, permission_mode: None, trust: true, create_folder: false }
}

fn get(m: &Manager, id: &str) -> SessionView {
    m.list().into_iter().find(|s| s.meta.id == id).expect("session exists")
}

async fn wait_status(m: &Manager, id: &str, want: Status) -> SessionView {
    for _ in 0..300 {
        let s = get(m, id);
        if s.meta.status == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let s = get(m, id);
    panic!("session {id} never reached {want:?}; it is {:?} ({:?})", s.meta.status, s.meta.status_detail);
}

fn texts(m: &Manager, id: &str) -> Vec<String> {
    m.events(id, 0)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::Claude { msg } if msg["type"] == "assistant" => msg["message"]["content"][0]["text"].as_str().map(String::from),
            _ => None,
        })
        .collect()
}

async fn wait_texts(m: &Manager, id: &str, n: usize) -> Vec<String> {
    for _ in 0..200 {
        let t = texts(m, id);
        if t.len() >= n {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    texts(m, id)
}

#[tokio::test]
async fn a_session_runs_a_turn_and_goes_idle() {
    let home = temp_home("sess-basic");
    let m = manager(&home, Config::default());
    let s = m.create(new(&home, "hello there\nsecond line")).unwrap();
    assert_eq!(s.meta.title, "hello there");
    let s = wait_status(&m, &s.meta.id, Status::Idle).await;
    assert!(s.meta.claude_session_id.as_deref().unwrap().starts_with("fake-"));
    assert_eq!(s.meta.model.as_deref(), Some("fake-model"));
    assert_eq!(texts(&m, &s.meta.id), vec!["echo: hello there\nsecond line"]);
    m.send(&s.meta.id, "again").unwrap();
    assert_eq!(wait_texts(&m, &s.meta.id, 2).await[1], "echo: again");
    let s = wait_status(&m, &s.meta.id, Status::Idle).await;
    assert!(s.pid.is_some(), "process stays up between turns");
    assert!(s.meta.outbox.is_empty(), "everything was delivered");
}

#[tokio::test]
async fn cost_counts_each_turn_once() {
    // Claude reports a running total per process; summing it would double count.
    let home = temp_home("sess-cost");
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "one")).unwrap().meta.id;
    wait_status(&m, &id, Status::Idle).await;
    m.send(&id, "two").unwrap();
    wait_texts(&m, &id, 2).await;
    let s = wait_status(&m, &id, Status::Idle).await;
    assert!((s.meta.cost_usd - 0.02).abs() < 1e-9, "cost {}", s.meta.cost_usd);
    let turn_costs: Vec<f64> = m
        .events(&id, 0)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::Claude { msg } if msg["type"] == "result" => msg["turn_cost_usd"].as_f64(),
            _ => None,
        })
        .collect();
    assert_eq!(turn_costs.len(), 2);
    assert!(turn_costs.iter().all(|c| (c - 0.01).abs() < 1e-9), "{turn_costs:?}");
}

#[tokio::test]
async fn permission_requests_wait_for_the_user() {
    let home = temp_home("sess-perm");
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "please use tool")).unwrap().meta.id;
    let s = wait_status(&m, &id, Status::NeedsYou).await;
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.pending[0].tool_name, "Bash");
    assert!(s.pending[0].can_remember);
    let rid = s.pending[0].request_id.clone();
    m.decide(&id, &rid, true, false, None).unwrap();
    wait_status(&m, &id, Status::Idle).await;
    assert!(texts(&m, &id).contains(&"allow: ls".to_string()));
    assert!(m.decide(&id, &rid, true, false, None).is_err(), "a decision cannot be sent twice");

    m.send(&id, "use tool again").unwrap();
    let s = wait_status(&m, &id, Status::NeedsYou).await;
    m.decide(&id, &s.pending[0].request_id, false, false, Some("no".into())).unwrap();
    wait_status(&m, &id, Status::Idle).await;
    m.send(&id, "use tool third").unwrap();
    let s = wait_status(&m, &id, Status::NeedsYou).await;
    m.decide(&id, &s.pending[0].request_id, true, true, None).unwrap();
    wait_status(&m, &id, Status::Idle).await;
    let t = wait_texts(&m, &id, 3).await;
    assert!(t.contains(&"deny: ls".to_string()), "{t:?}");
    // Claude suggested saving the rule to localSettings; hangar kept it to this session.
    assert!(t.contains(&"allow always(session): ls".to_string()), "{t:?}");
}

#[tokio::test]
async fn interrupting_withdraws_a_pending_request() {
    let home = temp_home("sess-interrupt");
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "use tool")).unwrap().meta.id;
    wait_status(&m, &id, Status::NeedsYou).await;
    m.interrupt(&id).unwrap();
    let s = wait_status(&m, &id, Status::Idle).await;
    assert!(s.pending.is_empty(), "the withdrawn request is gone");
    let cancelled = m.events(&id, 0).unwrap().into_iter().any(|e| matches!(e.body, EventBody::Decision { ref behavior, .. } if behavior == "cancelled"));
    assert!(cancelled);
    assert!(texts(&m, &id).contains(&"interrupted".to_string()));
}

#[tokio::test]
async fn untrusted_folders_need_confirmation_once() {
    let home = temp_home("sess-trust");
    let repo = temp_home("sess-trust-repo");
    let m = manager(&home, Config::default());
    let mut req = new(&repo, "hi");
    req.trust = false;
    let err = m.create(req).unwrap_err();
    assert!(err.downcast_ref::<NeedsTrust>().is_some(), "{err:#}");
    m.create(new(&repo, "hi")).unwrap(); // trust: true records it
    let mut again = new(&repo, "hi again");
    again.trust = false;
    m.create(again).expect("remembered as trusted");
}

#[tokio::test]
async fn the_budget_queues_sessions_instead_of_overcommitting() {
    let home = temp_home("sess-queue");
    let mut cfg = Config::default();
    cfg.budget.max_sessions = 1;
    cfg.budget.reclaim_idle_after_secs = 3600;
    let m = manager(&home, cfg);
    let a = m.create(new(&home, "first")).unwrap().meta.id;
    wait_status(&m, &a, Status::Idle).await;
    let b = m.create(new(&home, "second")).unwrap().meta.id;
    let sb = get(&m, &b);
    assert_eq!(sb.meta.status, Status::Queued);
    assert_eq!(sb.queue_position, Some(1));
    assert!(sb.meta.status_detail.unwrap_or_default().contains("limit 1"));
    m.stop(&a).unwrap();
    wait_status(&m, &a, Status::Stopped).await;
    wait_status(&m, &b, Status::Idle).await;
    assert_eq!(texts(&m, &b), vec!["echo: second"]);
}

#[tokio::test]
async fn idle_sessions_are_paused_to_make_room_and_resume_later() {
    let home = temp_home("sess-reclaim");
    let mut cfg = Config::default();
    cfg.budget.max_sessions = 1;
    cfg.budget.reclaim_idle_after_secs = 0;
    let m = manager(&home, cfg);
    let a = m.create(new(&home, "first")).unwrap().meta.id;
    wait_status(&m, &a, Status::Idle).await;
    let b = m.create(new(&home, "second")).unwrap().meta.id;
    // A is being paused right now; a message sent this instant must not be lost.
    m.send(&a, "sent while pausing").unwrap();
    wait_status(&m, &b, Status::Idle).await;
    let t = wait_texts(&m, &a, 2).await;
    assert_eq!(t, vec!["echo: first", "echo: sent while pausing"], "A resumed with the message");
    assert_eq!(get(&m, &b).meta.status, Status::Hibernated, "B made room in turn");
}

#[tokio::test]
async fn queued_messages_survive_a_daemon_restart() {
    let home = temp_home("sess-persist-queue");
    let mut cfg = Config::default();
    cfg.budget.max_sessions = 1;
    cfg.budget.reclaim_idle_after_secs = 3600;
    let (a, b);
    {
        let m = manager(&home, cfg.clone());
        a = m.create(new(&home, "first")).unwrap().meta.id;
        wait_status(&m, &a, Status::Idle).await;
        b = m.create(new(&home, "second")).unwrap().meta.id;
        m.send(&b, "and more").unwrap();
        assert_eq!(get(&m, &b).meta.status, Status::Queued);
        m.shutdown();
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let m = manager(&home, cfg);
    assert_eq!(get(&m, &a).meta.status, Status::Hibernated);
    wait_status(&m, &b, Status::Idle).await;
    let t = wait_texts(&m, &b, 2).await;
    assert_eq!(t, vec!["echo: second", "echo: and more"], "both queued messages were delivered");
}

#[tokio::test]
async fn a_conversation_that_cannot_resume_says_so_and_starts_fresh() {
    let home = temp_home("sess-bad-resume");
    let id;
    {
        let m = manager(&home, Config::default());
        id = m.create(new(&home, "hello")).unwrap().meta.id;
        wait_status(&m, &id, Status::Idle).await;
        m.shutdown();
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    // Pretend Claude lost the conversation.
    let meta_path = home.join("sessions").join(&id).join("meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta["claude_session_id"] = json!("bad");
    std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

    let m = manager(&home, Config::default());
    m.send(&id, "are you there?").unwrap();
    let s = wait_status(&m, &id, Status::Error).await;
    assert!(s.meta.status_detail.unwrap().contains("could not resume"));
    assert_eq!(s.meta.claude_session_id, None, "the dead id is dropped");
    assert_eq!(s.meta.outbox, vec!["are you there?"], "the message is kept");
    m.send(&id, "try again").unwrap();
    wait_status(&m, &id, Status::Idle).await;
    let t = wait_texts(&m, &id, 3).await;
    assert!(t.contains(&"echo: try again".to_string()), "{t:?}");
}

#[tokio::test]
async fn start_now_bypasses_the_queue() {
    let home = temp_home("sess-startnow");
    let mut cfg = Config::default();
    cfg.budget.max_sessions = 1;
    cfg.budget.reclaim_idle_after_secs = 3600;
    let m = manager(&home, cfg);
    let a = m.create(new(&home, "first")).unwrap().meta.id;
    wait_status(&m, &a, Status::Idle).await;
    let b = m.create(new(&home, "second")).unwrap().meta.id;
    assert_eq!(get(&m, &b).meta.status, Status::Queued);
    m.start_now(&b).unwrap();
    wait_status(&m, &b, Status::Idle).await;
}

#[tokio::test]
async fn stopped_sessions_resume_the_same_conversation() {
    let home = temp_home("sess-resume");
    let log = home.join("argv.log");
    // The fake reads FAKE_CLAUDE_LOG from its environment, which it inherits.
    unsafe { std::env::set_var("FAKE_CLAUDE_LOG", &log) };
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "remember me")).unwrap().meta.id;
    let s = wait_status(&m, &id, Status::Idle).await;
    let claude_id = s.meta.claude_session_id.clone().unwrap();
    m.stop(&id).unwrap();
    wait_status(&m, &id, Status::Stopped).await;
    m.send(&id, "are you back?").unwrap();
    wait_texts(&m, &id, 2).await;
    wait_status(&m, &id, Status::Idle).await;
    let argv = std::fs::read_to_string(&log).unwrap();
    assert!(argv.lines().any(|l| l.contains(&format!("--resume={claude_id}"))), "{argv}");
}

#[tokio::test]
async fn the_model_switches_mid_session_and_survives_a_resume() {
    use hangar::sessions::Update;
    let home = temp_home("sess-model");
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "which model")).unwrap().meta.id;
    wait_status(&m, &id, Status::Idle).await;
    let model = |name: &str| Update { model: Some(name.into()), ..Default::default() };
    assert!(m.update(&id, model("-x")).is_err(), "model names cannot be flags");
    assert_eq!(m.update(&id, model("haiku")).unwrap().meta.model.as_deref(), Some("haiku"));
    m.send(&id, "which model").unwrap();
    assert_eq!(wait_texts(&m, &id, 2).await, vec!["model: fake-model", "model: haiku"], "the running process switched");
    wait_status(&m, &id, Status::Idle).await;
    m.stop(&id).unwrap();
    wait_status(&m, &id, Status::Stopped).await;
    m.send(&id, "which model").unwrap();
    assert_eq!(wait_texts(&m, &id, 3).await[2], "model: haiku", "a resumed session starts with the new model");
    wait_status(&m, &id, Status::Idle).await;
    assert_eq!(m.update(&id, model("")).unwrap().meta.model, None, "an empty name is Claude Code's default");
    m.send(&id, "which model").unwrap();
    assert_eq!(wait_texts(&m, &id, 4).await[3], "model: fake-model");
}

#[tokio::test]
async fn a_crash_is_reported_with_its_exit_code() {
    let home = temp_home("sess-crash");
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "crash now")).unwrap().meta.id;
    let s = wait_status(&m, &id, Status::Error).await;
    assert!(s.meta.status_detail.unwrap().contains("code 5"));
    assert!(s.pid.is_none());
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let home = temp_home("sess-bad");
    let m = manager(&home, Config::default());
    assert!(m.create(new(&home.join("missing"), "x")).is_err());
    assert!(m.create(new(&home, "   ")).is_err());
    let mut n = new(&home, "x");
    n.permission_mode = Some("yolo".into());
    assert!(m.create(n).is_err());
    assert!(m.events("../../etc", 0).is_err(), "unknown ids never reach the disk");
}

#[tokio::test]
async fn sessions_survive_a_daemon_restart() {
    let home = temp_home("sess-persist");
    let id;
    {
        let m = manager(&home, Config::default());
        id = m.create(new(&home, "keep this")).unwrap().meta.id;
        wait_status(&m, &id, Status::Idle).await;
        m.shutdown();
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let m = manager(&home, Config::default());
    assert_eq!(get(&m, &id).meta.status, Status::Hibernated, "a live session comes back resumable");
    assert_eq!(texts(&m, &id), vec!["echo: keep this"]);
}

// ------------------------------------------------------------------ API

struct Ui {
    d: Daemon,
    base: String,
    http: reqwest::Client,
}

async fn ui_daemon(tag: &str) -> Ui {
    let home = temp_home(tag);
    let d = Daemon::start(DaemonOptions { home: home.clone(), config: Config::default(), ui_port: Some(0) }).await.unwrap();
    d.sessions.set_claude_bin(Some(fake_claude()));
    d.sessions.set_claude_json(home.join("no-claude.json"));
    let base = format!("http://127.0.0.1:{}", d.info.ui_port.unwrap());
    Ui { d, base, http: reqwest::Client::new() }
}

impl Ui {
    async fn token(&self) -> String {
        let code = self.d.pairing.issue();
        let r: Value = self.http.post(format!("{}/api/pair", self.base)).json(&json!({"code": code})).send().await.unwrap().json().await.unwrap();
        r["token"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn pairing_codes_unlock_the_api_once() {
    let ui = ui_daemon("api-pair").await;
    let url = format!("{}/api/state", ui.base);
    assert_eq!(ui.http.get(&url).send().await.unwrap().status(), 401, "no token");
    assert_eq!(ui.http.get(&url).header("x-hangar-token", "guess").send().await.unwrap().status(), 401);
    let code = ui.d.pairing.issue();
    let pair = |c: String| ui.http.post(format!("{}/api/pair", ui.base)).json(&json!({"code": c})).send();
    let first = pair(code.clone()).await.unwrap();
    assert_eq!(first.status(), 200);
    let token = first.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!(pair(code).await.unwrap().status(), 401, "codes work once");
    assert_ne!(token, ui.d.info.token, "the UI token is not the daemon token");
    let ok = ui.http.get(&url).header("x-hangar-token", &token).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    let state: Value = ok.json().await.unwrap();
    assert!(state["machine"]["total_bytes"].as_u64().unwrap() > 0);
    ui.d.shutdown().await;
}

#[tokio::test]
async fn the_folder_picker_lists_folders_and_a_new_folder_is_made_when_its_session_starts() {
    let ui = ui_daemon("api-folders").await;
    let root = temp_home("api-folders-tree");
    std::fs::create_dir_all(root.join("web-app")).unwrap();
    std::fs::write(root.join("notes.txt"), "files are never listed").unwrap();
    let url = format!("{}/api/folders", ui.base);
    let list = |token: Option<String>, path: String| {
        let mut r = ui.http.get(&url).query(&[("path", path)]);
        if let Some(t) = token {
            r = r.header("x-hangar-token", t);
        }
        r.send()
    };
    assert_eq!(list(None, root.display().to_string()).await.unwrap().status(), 401, "only for the signed-in app");
    let token = ui.token().await;
    let listing: Value = list(Some(token.clone()), root.display().to_string()).await.unwrap().json().await.unwrap();
    assert_eq!(listing["exists"], true);
    let names: Vec<&str> = listing["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["web-app"], "folders only, never files");
    let top: Value = list(Some(token.clone()), String::new()).await.unwrap().json().await.unwrap();
    assert!(top["recent"].is_array(), "the top level brings fresh recent folders");
    let search = ui.http.get(format!("{url}/search")).query(&[("q", "web-app")]);
    assert_eq!(search.try_clone().unwrap().send().await.unwrap().status(), 401);
    let found: Value = search.header("x-hangar-token", &token).send().await.unwrap().json().await.unwrap();
    assert!(found["entries"].is_array());

    let start = |body: Value| ui.http.post(format!("{}/api/sessions", ui.base)).header("x-hangar-token", &token).json(&body).send();
    let new_dir = root.join("todo-app");
    let cwd = new_dir.display().to_string();
    let empty = start(json!({"cwd": cwd, "prompt": " ", "create_folder": true})).await.unwrap();
    assert_eq!(empty.status(), 400);
    assert!(!new_dir.exists(), "nothing is made for a session that does not start");
    let deep = start(json!({"cwd": root.join("no").join("such").display().to_string(), "prompt": "hi", "create_folder": true})).await.unwrap();
    assert_eq!(deep.status(), 400, "one level at a time");
    let made = start(json!({"cwd": cwd, "prompt": "hi", "create_folder": true})).await.unwrap();
    assert_eq!(made.status(), 200, "a folder hangar just made needs no trust prompt");
    assert!(new_dir.is_dir());
    let again = start(json!({"cwd": cwd, "prompt": "hi again", "create_folder": true})).await.unwrap();
    assert_eq!(again.status(), 200, "an existing folder is used as it is");
    ui.d.shutdown().await;
}

#[tokio::test]
async fn foreign_hosts_origins_and_websockets_are_refused() {
    let ui = ui_daemon("api-origin").await;
    let token = ui.token().await;
    let url = format!("{}/api/state", ui.base);
    let rebound = ui.http.get(&url).header("x-hangar-token", &token).header("host", "evil.example").send().await.unwrap();
    assert_eq!(rebound.status(), 403, "DNS rebinding");
    let cross = ui.http.get(&url).header("x-hangar-token", &token).header("origin", "https://evil.example").send().await.unwrap();
    assert_eq!(cross.status(), 403, "cross-origin");
    // WebSocket handshakes: the token rides in a subprotocol.
    let ws = |protocols: &str| {
        ui.http
            .get(format!("{}/api/ws", ui.base))
            .header("origin", &ui.base)
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-protocol", protocols.to_string())
            .send()
    };
    assert_eq!(ws("hangar").await.unwrap().status(), 401);
    let accepted = ws(&format!("hangar, token.{token}")).await.unwrap();
    assert_eq!(accepted.status(), 101);
    assert_eq!(accepted.headers()["sec-websocket-protocol"], "hangar");
    ui.d.shutdown().await;
}

#[tokio::test]
async fn pages_carry_a_strict_content_security_policy() {
    let ui = ui_daemon("api-csp").await;
    let r = ui.http.get(format!("{}/", ui.base)).send().await.unwrap();
    let csp = r.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.contains("img-src 'self' data:") && csp.contains("connect-src 'self'"), "{csp}");
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    ui.d.shutdown().await;
}

#[tokio::test]
async fn sessions_run_end_to_end_through_the_api() {
    let ui = ui_daemon("api-e2e").await;
    let token = ui.token().await;
    let repo = temp_home("api-e2e-repo");
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(repo.join(".claude/settings.json"), r#"{"permissions":{"allow":["Bash(*)"]}}"#).unwrap();
    let post = |path: &str, body: Value| ui.http.post(format!("{}{path}", ui.base)).header("x-hangar-token", &token).json(&body).send();
    // The folder pre-approves every shell command, so hangar asks first.
    let trust: Value = ui
        .http
        .get(format!("{}/api/trust", ui.base))
        .query(&[("cwd", repo.display().to_string())])
        .header("x-hangar-token", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(trust["trusted"], false);
    assert_eq!(trust["allow_rules"], json!(["Bash(*)"]));
    let refused = post("/api/sessions", json!({"cwd": repo.display().to_string(), "prompt": "hi"})).await.unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(refused.json::<Value>().await.unwrap()["trust"]["allow_rules"], json!(["Bash(*)"]));
    let created = post("/api/sessions", json!({"cwd": repo.display().to_string(), "prompt": "hi", "trust": true})).await.unwrap();
    assert_eq!(created.status(), 200);
    let id = created.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();
    for _ in 0..200 {
        let v: Value = ui.http.get(format!("{}/api/state", ui.base)).header("x-hangar-token", &token).send().await.unwrap().json().await.unwrap();
        if v["sessions"][0]["status"] == "idle" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let events: Value =
        ui.http.get(format!("{}/api/sessions/{id}/events", ui.base)).header("x-hangar-token", &token).send().await.unwrap().json().await.unwrap();
    assert!(events.as_array().unwrap().iter().any(|e| e["kind"] == "claude" && e["msg"]["result"] == "echo: hi"));
    ui.d.shutdown().await;
}
