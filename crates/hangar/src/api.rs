//! The UI server: the web app, a JSON API and a WebSocket of live updates.
//!
//! Binds to 127.0.0.1 only. Requests must carry the `hangar` cookie, which
//! `/auth?t=<token>` sets from the daemon's token (only the current user can
//! read it). The Host header must be the loopback address we listen on, which
//! stops DNS rebinding, and state-changing requests must come from our own
//! origin, which stops other sites from driving your agents.

use crate::pool::Pool;
use crate::procs;
use crate::sessions::{Manager, NewSession};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<Manager>,
    pub pool: Arc<Pool>,
    pub token: String,
    pub port: u16,
}

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

pub async fn serve(listener: tokio::net::TcpListener, state: AppState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let api = Router::new()
        .route("/state", get(get_state))
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", axum::routing::patch(update_session).delete(delete_session))
        .route("/sessions/{id}/events", get(get_events))
        .route("/sessions/{id}/messages", post(send_message))
        .route("/sessions/{id}/permissions/{request_id}", post(decide))
        .route("/sessions/{id}/interrupt", post(interrupt))
        .route("/sessions/{id}/stop", post(stop))
        .route("/sessions/{id}/start", post(start_now))
        .route("/servers/{id}/stop", post(stop_server))
        .route("/ws", get(ws))
        .layer(middleware::from_fn_with_state(state.clone(), guard));
    let app = Router::new()
        .route("/auth", get(auth))
        .nest("/api", api)
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(state.clone(), host_guard))
        .with_state(state);
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            while !*shutdown.borrow() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
}

fn allowed_hosts(port: u16) -> [String; 2] {
    [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
}

/// Rejects requests whose Host is not our loopback address (DNS rebinding).
async fn host_guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    if !allowed_hosts(st.port).iter().any(|h| h == host) {
        return (StatusCode::FORBIDDEN, "wrong host").into_response();
    }
    next.run(req).await
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').map(str::trim).find_map(|c| c.strip_prefix("hangar=")).map(String::from)
}

/// API requests need the cookie; writes and WebSockets also need our origin.
async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if cookie_token(req.headers()).as_deref() != Some(st.token.as_str()) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "open hangar with `hangar up`"}))).into_response();
    }
    let is_ws = req.headers().get(header::UPGRADE).is_some();
    if req.method() != Method::GET || is_ws {
        let origin = req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok());
        let ok = match origin {
            Some(o) => allowed_hosts(st.port).iter().any(|h| o == format!("http://{h}")),
            None => !is_ws, // non-browser clients send no Origin
        };
        if !ok {
            return (StatusCode::FORBIDDEN, Json(json!({"error": "cross-origin request refused"}))).into_response();
        }
    }
    next.run(req).await
}

#[derive(Deserialize)]
struct AuthQuery {
    t: String,
}

async fn auth(State(st): State<AppState>, Query(q): Query<AuthQuery>) -> Response {
    if q.t != st.token {
        return (StatusCode::UNAUTHORIZED, "bad token; run `hangar up` again").into_response();
    }
    let cookie = format!("hangar={}; Path=/; HttpOnly; SameSite=Strict", st.token);
    ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response()
}

async fn static_file(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let file = Assets::get(path).map(|f| (path, f)).or_else(|| Assets::get("index.html").map(|f| ("index.html", f)));
    match file {
        Some((p, f)) => {
            let mime = mime_guess::from_path(p).first_or_octet_stream();
            let cache = if p.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
            ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())], f.data.into_owned()).into_response()
        }
        None => (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")], "<h1>hangar</h1><p>The web UI was not built. Run <code>npm ci && npm run build</code> in <code>web/</code>, then rebuild hangar.</p>").into_response(),
    }
}

fn err(status: StatusCode, e: impl std::fmt::Display) -> Response {
    (status, Json(json!({"error": e.to_string()}))).into_response()
}

/// Everything the UI shows outside the transcript.
pub async fn system_snapshot(st: &AppState) -> Value {
    let stats = st.pool.stats().await;
    let snap = procs::snapshot();
    let mut saved: u64 = 0;
    let servers: Vec<Value> = stats
        .into_iter()
        .map(|s| {
            let mem = s.pid.map(|pid| procs::Usage::of(procs::tree(&snap, pid))).unwrap_or_default();
            let attached = s.sessions.len() as u64;
            if attached > 1 && s.share != crate::policy::Share::Session {
                saved += (attached - 1) * mem.private_bytes;
            }
            let mut v = serde_json::to_value(&s).unwrap_or(Value::Null);
            v["memory"] = serde_json::to_value(mem).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.remove("stderr_tail");
            }
            v
        })
        .collect();
    let daemon = snap.iter().find(|p| p.pid == std::process::id()).map(|p| p.private_bytes).unwrap_or(0);
    json!({
        "machine": st.manager.machine(),
        "servers": servers,
        "saved_bytes": saved,
        "daemon_bytes": daemon,
        "version": env!("CARGO_PKG_VERSION"),
    })
}

async fn get_state(State(st): State<AppState>) -> Response {
    let mut sys = system_snapshot(&st).await;
    sys["sessions"] = serde_json::to_value(st.manager.list()).unwrap_or(Value::Null);
    sys["projects"] = json!(st.manager.recent_projects());
    sys["home"] = json!(dirs_home());
    Json(sys).into_response()
}

fn dirs_home() -> String {
    std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default()
}

#[derive(Deserialize)]
struct CreateBody {
    cwd: String,
    prompt: String,
    title: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
}

async fn create_session(State(st): State<AppState>, Json(b): Json<CreateBody>) -> Response {
    match st.manager.create(NewSession { cwd: b.cwd, prompt: b.prompt, title: b.title, model: b.model, permission_mode: b.permission_mode }) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct UpdateBody {
    title: Option<String>,
    permission_mode: Option<String>,
}

async fn update_session(State(st): State<AppState>, Path(id): Path<String>, Json(b): Json<UpdateBody>) -> Response {
    match st.manager.update(&id, b.title, b.permission_mode) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn delete_session(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    result(st.manager.remove(&id))
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: u64,
}

async fn get_events(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> Response {
    match st.manager.events(&id, q.after) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn send_message(State(st): State<AppState>, Path(id): Path<String>, Json(b): Json<MessageBody>) -> Response {
    result(st.manager.send(&id, &b.text))
}

#[derive(Deserialize)]
struct DecideBody {
    behavior: String,
    #[serde(default)]
    message: Option<String>,
}

async fn decide(State(st): State<AppState>, Path((id, request_id)): Path<(String, String)>, Json(b): Json<DecideBody>) -> Response {
    let (allow, always) = match b.behavior.as_str() {
        "allow" => (true, false),
        "allow_always" => (true, true),
        "deny" => (false, false),
        other => return err(StatusCode::BAD_REQUEST, format!("unknown behavior {other}")),
    };
    result(st.manager.decide(&id, &request_id, allow, always, b.message))
}

async fn interrupt(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    result(st.manager.interrupt(&id))
}

async fn stop(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    result(st.manager.stop(&id))
}

async fn start_now(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    result(st.manager.start_now(&id))
}

async fn stop_server(State(st): State<AppState>, Path(id): Path<u64>) -> Response {
    if st.pool.stop(id).await { Json(json!({"ok": true})).into_response() } else { err(StatusCode::NOT_FOUND, "no such server") }
}

fn result(r: anyhow::Result<()>) -> Response {
    match r {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn ws(State(st): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| live(socket, st))
}

/// Pushes session changes as they happen and a system snapshot every 2 s.
async fn live(mut socket: WebSocket, st: AppState) {
    let mut rx = st.manager.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let text = match ev {
                    Ok(ev) => serde_json::to_string(&ev).unwrap_or_default(),
                    // Fell behind: tell the client to refetch everything.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => json!({"type": "resync"}).to_string(),
                    Err(_) => break,
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            _ = tick.tick() => {
                let mut sys = system_snapshot(&st).await;
                sys["type"] = json!("system");
                if socket.send(Message::Text(sys.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}
