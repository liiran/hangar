//! The UI server: the web app, a JSON API and a WebSocket of live updates.
//!
//! Security model (the API can start agents, so it must be reachable only by
//! the user's own window):
//! * binds to 127.0.0.1, and the Host header must be that address, which
//!   stops DNS rebinding;
//! * every API call carries the UI token in `x-hangar-token` (the WebSocket
//!   carries it as a subprotocol). The page gets the token by redeeming a
//!   one-time pairing code that `hangar up` puts in the URL fragment, and
//!   keeps it in storage scoped to this exact origin, so pages on other
//!   ports cannot read it. There is no cookie for other ports to receive;
//! * requests from any other origin are refused;
//! * pages are served with a strict Content-Security-Policy (no remote
//!   scripts, images or connections).

use crate::daemon::Pairing;
use crate::pool::Pool;
use crate::procs;
use crate::sessions::{Manager, NeedsTrust, NewSession};
use crate::util::constant_time_eq;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; \
                   font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'; object-src 'none'";

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<Manager>,
    pub pool: Arc<Pool>,
    token: Arc<str>,
    pairing: Arc<Pairing>,
    pub port: u16,
    snapshot: Arc<tokio::sync::Mutex<Option<(Instant, Value)>>>,
}

impl AppState {
    pub fn new(manager: Arc<Manager>, pool: Arc<Pool>, token: String, pairing: Arc<Pairing>, port: u16) -> AppState {
        AppState { manager, pool, token: token.into(), pairing, port, snapshot: Default::default() }
    }
}

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

pub async fn serve(listener: tokio::net::TcpListener, state: AppState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let api = Router::new()
        .route("/state", get(get_state))
        .route("/trust", get(get_trust))
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
        .layer(middleware::from_fn_with_state(state.clone(), require_token));
    let app = Router::new()
        .route("/api/pair", post(pair))
        .nest("/api", api)
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(state.clone(), same_origin_only))
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

fn our_origins(port: u16) -> [String; 2] {
    [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
}

/// Host must be our loopback address (DNS rebinding); an Origin, when sent,
/// must be ours (cross-site requests). Adds security headers to every answer.
async fn same_origin_only(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let hosts = our_origins(st.port);
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    if !hosts.iter().any(|h| h == host) {
        return (StatusCode::FORBIDDEN, "wrong host").into_response();
    }
    if let Some(origin) = req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok())
        && !hosts.iter().any(|h| origin == format!("http://{h}"))
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "cross-origin request refused"}))).into_response();
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    res
}

/// The UI token from the header, or from a WebSocket subprotocol `token.<t>`.
fn presented_token(headers: &HeaderMap) -> Option<String> {
    if let Some(t) = headers.get("x-hangar-token").and_then(|v| v.to_str().ok()) {
        return Some(t.to_string());
    }
    let protocols = headers.get(header::SEC_WEBSOCKET_PROTOCOL)?.to_str().ok()?;
    protocols.split(',').map(str::trim).find_map(|p| p.strip_prefix("token.")).map(String::from)
}

async fn require_token(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let ok = presented_token(req.headers()).is_some_and(|t| constant_time_eq(&t, &st.token));
    if !ok {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "open hangar with `hangar up`"}))).into_response();
    }
    if req.headers().get(header::UPGRADE).is_some() && req.headers().get(header::ORIGIN).is_none() {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "WebSocket without an origin"}))).into_response();
    }
    next.run(req).await
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
}

/// Trades a one-time code from `hangar up` for the UI token.
async fn pair(State(st): State<AppState>, Json(b): Json<PairBody>) -> Response {
    if st.pairing.redeem(&b.code) {
        Json(json!({"token": &*st.token})).into_response()
    } else {
        err(StatusCode::UNAUTHORIZED, "this link has expired or was already used; run `hangar up` again")
    }
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
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html")],
            "<h1>hangar</h1><p>The web UI was not built. Run <code>npm ci && npm run build</code> in <code>web/</code>, then rebuild hangar.</p>",
        )
            .into_response(),
    }
}

fn err(status: StatusCode, e: impl std::fmt::Display) -> Response {
    (status, Json(json!({"error": e.to_string()}))).into_response()
}

/// Everything the UI shows outside the transcript. Computed at most every
/// 1.5 s and shared by all windows.
pub async fn system_snapshot(st: &AppState) -> Value {
    let mut cache = st.snapshot.lock().await;
    if let Some((at, v)) = cache.as_ref()
        && at.elapsed() < Duration::from_millis(1500)
    {
        return v.clone();
    }
    let stats = st.pool.stats().await;
    let snap = tokio::task::spawn_blocking(procs::snapshot).await.unwrap_or_default();
    let mem_of = |pid: u32| snap.iter().find(|p| p.pid == pid).map_or(0, |p| p.private_bytes);
    // What sharing saves: each extra session on a shared server would have
    // started its own copy. Multi-session servers keep per-session state
    // (browser contexts) inside the shared process, so they are not counted.
    // The shims and the daemon itself are what sharing costs.
    let mut duplicated: u64 = 0;
    let mut overhead: u64 = 0;
    let servers: Vec<Value> = stats
        .into_iter()
        .map(|s| {
            let mem = s.pid.map(|pid| procs::Usage::of(procs::tree(&snap, pid))).unwrap_or_default();
            let attached = s.sessions.len() as u64;
            if attached > 1 && matches!(s.share, crate::policy::Share::Global | crate::policy::Share::Project) {
                duplicated += (attached - 1) * mem.private_bytes;
            }
            overhead += s.sessions.iter().map(|c| mem_of(c.shim_pid)).sum::<u64>();
            let mut v = serde_json::to_value(&s).unwrap_or(Value::Null);
            v["memory"] = serde_json::to_value(mem).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.remove("stderr_tail");
            }
            v
        })
        .collect();
    let daemon = mem_of(std::process::id());
    let saved = if duplicated > 0 { duplicated.saturating_sub(overhead + daemon) } else { 0 };
    let v = json!({
        "machine": st.manager.machine(),
        "servers": servers,
        "saved_bytes": saved,
        "daemon_bytes": daemon,
        "version": env!("CARGO_PKG_VERSION"),
    });
    *cache = Some((Instant::now(), v.clone()));
    v
}

async fn get_state(State(st): State<AppState>) -> Response {
    let mut sys = system_snapshot(&st).await;
    sys["sessions"] = serde_json::to_value(st.manager.list()).unwrap_or(Value::Null);
    sys["projects"] = json!(st.manager.recent_projects());
    sys["home"] = json!(std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default());
    Json(sys).into_response()
}

#[derive(Deserialize)]
struct TrustQuery {
    cwd: String,
}

async fn get_trust(State(st): State<AppState>, Query(q): Query<TrustQuery>) -> Response {
    let cwd = std::path::PathBuf::from(q.cwd.trim());
    if !cwd.is_dir() {
        return err(StatusCode::BAD_REQUEST, format!("{} is not a folder", cwd.display()));
    }
    Json(st.manager.trust(&cwd)).into_response()
}

#[derive(Deserialize)]
struct CreateBody {
    cwd: String,
    prompt: String,
    title: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    #[serde(default)]
    trust: bool,
}

async fn create_session(State(st): State<AppState>, Json(b): Json<CreateBody>) -> Response {
    let req = NewSession { cwd: b.cwd, prompt: b.prompt, title: b.title, model: b.model, permission_mode: b.permission_mode, trust: b.trust };
    match st.manager.create(req) {
        Ok(v) => Json(v).into_response(),
        Err(e) => match e.downcast_ref::<NeedsTrust>() {
            Some(NeedsTrust(info)) => (StatusCode::CONFLICT, Json(json!({"error": e.to_string(), "trust": info}))).into_response(),
            None => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
        },
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
    upgrade.protocols(["hangar"]).on_upgrade(move |socket| live(socket, st))
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
