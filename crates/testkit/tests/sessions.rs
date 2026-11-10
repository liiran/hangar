//! Session manager and UI API tests, driven by a fake `claude`.

mod common;

use common::*;
use hangar::config::Config;
use hangar::daemon::{Daemon, DaemonOptions};
use hangar::sessions::{EventBody, Manager, NewSession, SessionView, Status};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

fn fake_claude() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_fake-claude").into()
}

fn manager(home: &std::path::Path, config: Config) -> Arc<Manager> {
    let m = Manager::start(home, &config);
    m.set_claude_bin(Some(fake_claude()));
    m
}

fn new(cwd: &std::path::Path, prompt: &str) -> NewSession {
    NewSession { cwd: cwd.display().to_string(), prompt: prompt.into(), title: None, model: None, permission_mode: None }
}

fn get(m: &Manager, id: &str) -> SessionView {
    m.list().into_iter().find(|s| s.meta.id == id).expect("session exists")
}

async fn wait_status(m: &Manager, id: &str, want: Status) -> SessionView {
    for _ in 0..200 {
        let s = get(m, id);
        if s.meta.status == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {id} never reached {want:?}; it is {:?} ({:?})", get(m, id).meta.status, get(m, id).meta.status_detail);
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

#[tokio::test]
async fn a_session_runs_a_turn_and_goes_idle() {
    let home = temp_home("sess-basic");
    let m = manager(&home, Config::default());
    let s = m.create(new(&home, "hello there\nsecond line")).unwrap();
    assert_eq!(s.meta.title, "hello there");
    let s = wait_status(&m, &s.meta.id, Status::Idle).await;
    assert!(s.meta.claude_session_id.as_deref().unwrap().starts_with("fake-"));
    assert_eq!(s.meta.model.as_deref(), Some("fake-model"));
    assert!((s.meta.cost_usd - 0.01).abs() < 1e-9);
    assert_eq!(texts(&m, &s.meta.id), vec!["echo: hello there\nsecond line"]);
    m.send(&s.meta.id, "again").unwrap();
    wait_status(&m, &s.meta.id, Status::Idle).await;
    for _ in 0..40 {
        if texts(&m, &s.meta.id).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(texts(&m, &s.meta.id)[1], "echo: again");
    assert!(s.pid.is_some(), "process stays up between turns");
}

#[tokio::test]
async fn permission_requests_wait_for_the_user() {
    let home = temp_home("sess-perm");
    let m = manager(&home, Config::default());
    let id = m.create(new(&home, "please use tool")).unwrap().meta.id;
    let s = wait_status(&m, &id, Status::NeedsYou).await;
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.pending[0].tool_name, "Bash");
    let rid = s.pending[0].request_id.clone();
    m.decide(&id, &rid, true, false, None).unwrap();
    wait_status(&m, &id, Status::Idle).await;
    assert!(texts(&m, &id).contains(&"allow: ls".to_string()));
    assert!(m.decide(&id, &rid, true, false, None).is_err(), "a decision cannot be sent twice");

    // Deny, and "always allow" (which sends the suggested rules).
    m.send(&id, "use tool again").unwrap();
    let s = wait_status(&m, &id, Status::NeedsYou).await;
    m.decide(&id, &s.pending[0].request_id, false, false, Some("no".into())).unwrap();
    wait_status(&m, &id, Status::Idle).await;
    m.send(&id, "use tool third").unwrap();
    let s = wait_status(&m, &id, Status::NeedsYou).await;
    m.decide(&id, &s.pending[0].request_id, true, true, None).unwrap();
    wait_status(&m, &id, Status::Idle).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let t = texts(&m, &id);
    assert!(t.contains(&"deny: ls".to_string()), "{t:?}");
    assert!(t.contains(&"allow always: ls".to_string()), "{t:?}");
}

#[tokio::test]
async fn the_budget_queues_sessions_instead_of_overcommitting() {
    let home = temp_home("sess-queue");
    let mut cfg = Config::default();
    cfg.budget.max_sessions = 1;
    let m = manager(&home, cfg);
    let a = m.create(new(&home, "first")).unwrap().meta.id;
    wait_status(&m, &a, Status::Idle).await;
    let b = m.create(new(&home, "second")).unwrap().meta.id;
    let sb = get(&m, &b);
    assert_eq!(sb.meta.status, Status::Queued);
    assert_eq!(sb.queue_position, Some(1));
    assert!(sb.meta.status_detail.unwrap_or_default().contains("limit 1"));
    // Freeing the slot starts the queued session.
    m.stop(&a).unwrap();
    wait_status(&m, &a, Status::Stopped).await;
    wait_status(&m, &b, Status::Idle).await;
    assert_eq!(texts(&m, &b), vec!["echo: second"]);
}

#[tokio::test]
async fn start_now_bypasses_the_queue() {
    let home = temp_home("sess-startnow");
    let mut cfg = Config::default();
    cfg.budget.max_sessions = 1;
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
    wait_status(&m, &id, Status::Idle).await;
    let argv = std::fs::read_to_string(&log).unwrap();
    assert!(argv.lines().any(|l| l.contains(&format!("--resume={claude_id}"))), "{argv}");
    assert_eq!(get(&m, &id).meta.claude_session_id.as_deref(), Some(claude_id.as_str()));
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
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let m = manager(&home, Config::default());
    let s = get(&m, &id);
    assert_eq!(s.meta.status, Status::Hibernated, "a live session comes back resumable");
    assert_eq!(texts(&m, &id), vec!["echo: keep this"]);
}

// ------------------------------------------------------------------ API

async fn ui_daemon(tag: &str) -> (Daemon, std::path::PathBuf, reqwest::Client, String) {
    let home = temp_home(tag);
    let d = Daemon::start(DaemonOptions { home: home.clone(), config: Config::default(), ui_port: Some(0) }).await.unwrap();
    d.sessions.set_claude_bin(Some(fake_claude()));
    let base = format!("http://127.0.0.1:{}", d.info.ui_port.unwrap());
    (d, home, reqwest::Client::new(), base)
}

#[tokio::test]
async fn api_requires_the_cookie_and_our_origin() {
    let (d, home, http, base) = ui_daemon("api-auth").await;
    let token = &d.info.token;
    assert_eq!(http.get(format!("{base}/api/state")).send().await.unwrap().status(), 401);
    let bad = http.get(format!("{base}/auth?t=wrong")).send().await.unwrap();
    assert_eq!(bad.status(), 401);
    let cookie = format!("hangar={token}");
    let ok = http.get(format!("{base}/api/state")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    let state: Value = ok.json().await.unwrap();
    assert!(state["machine"]["total_bytes"].as_u64().unwrap() > 0);
    // DNS rebinding: a foreign Host header is refused even with the cookie.
    let rebound = http.get(format!("{base}/api/state")).header("cookie", &cookie).header("host", "evil.example:80").send().await.unwrap();
    assert_eq!(rebound.status(), 403);
    // Cross-site writes are refused.
    let body = json!({"cwd": home.display().to_string(), "prompt": "hi"});
    let csrf = http.post(format!("{base}/api/sessions")).header("cookie", &cookie).header("origin", "https://evil.example").json(&body).send().await.unwrap();
    assert_eq!(csrf.status(), 403);
    // Our own origin works end to end.
    let created = http.post(format!("{base}/api/sessions")).header("cookie", &cookie).header("origin", &base).json(&body).send().await.unwrap();
    assert_eq!(created.status(), 200);
    let s: Value = created.json().await.unwrap();
    let id = s["id"].as_str().unwrap().to_string();
    for _ in 0..200 {
        let v: Value = http.get(format!("{base}/api/state")).header("cookie", &cookie).send().await.unwrap().json().await.unwrap();
        if v["sessions"][0]["status"] == "idle" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let events: Value = http.get(format!("{base}/api/sessions/{id}/events")).header("cookie", &cookie).send().await.unwrap().json().await.unwrap();
    assert!(events.as_array().unwrap().iter().any(|e| e["kind"] == "user" && e["text"] == "hi"));
    d.shutdown().await;
}

#[tokio::test]
async fn auth_sets_an_http_only_strict_cookie() {
    let (d, _home, _http, base) = ui_daemon("api-cookie").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = http.get(format!("{base}/auth?t={}", d.info.token)).send().await.unwrap();
    assert!(r.status().is_redirection());
    let cookie = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    d.shutdown().await;
}
