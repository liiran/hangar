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
//! * requests from any other origin are refused, including ones a browser
//!   marks as cross-site (`Sec-Fetch-Site`), so another site cannot even
//!   load an icon to learn that hangar is running;
//! * pages are served with a strict Content-Security-Policy (no remote
//!   scripts, images or connections), may not be framed, and API answers
//!   are never cached.

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

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; \
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
        .route("/folders", get(list_folders))
        .route("/folders/search", get(search_folders))
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", axum::routing::patch(update_session).delete(delete_session))
        .route("/sessions/{id}/events", get(get_events))
        .route("/sessions/{id}/messages", post(send_message))
        .route("/sessions/{id}/permissions/{request_id}", post(decide))
        .route("/sessions/{id}/interrupt", post(interrupt))
        .route("/sessions/{id}/stop", post(stop))
        .route("/sessions/{id}/start", post(start_now))
        .route("/servers/{id}/stop", post(stop_server))
        .route("/sharing", post(set_sharing))
        .route("/ws", get(ws))
        // An unknown API path is an error, not the app's page.
        .fallback(|| async { err(StatusCode::NOT_FOUND, "no such API path") })
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

/// Whether a request may be answered at all, judged by where the browser
/// says it comes from. `host` is the only name hangar answers to.
///
/// * Host must be exactly our loopback address and port (DNS rebinding).
///   `localhost` is not accepted: only 127.0.0.1 is bound, so another
///   program could be the one answering on `localhost`. There must be one
///   Host header, and a request line that names a site (`target`, from
///   `GET http://site/path`) must name ours: a second name is one that
///   some other reader of the request might believe instead.
/// * An Origin, when sent, must be ours. One that cannot be read is refused.
/// * `Sec-Fetch-Site`, which browsers set themselves: `same-origin` and
///   `none` (typed, bookmarked, opened by `hangar up`) pass. Another site,
///   or another port on this machine, may only open the page itself as a
///   top-level navigation; it gets no API, script, image or frame.
fn request_allowed(headers: &HeaderMap, target: Option<&str>, host: &str, is_api: bool) -> Result<(), &'static str> {
    let text = |name: &header::HeaderName| headers.get(name).map(|v| v.to_str().ok());
    if headers.get_all(header::HOST).iter().count() != 1 || text(&header::HOST).flatten() != Some(host) || target.is_some_and(|t| t != host) {
        return Err("wrong host");
    }
    match text(&header::ORIGIN) {
        None => {}
        Some(Some(origin)) if origin == format!("http://{host}") => {}
        Some(_) => return Err("cross-origin request refused"),
    }
    match headers.get("sec-fetch-site").map(|v| v.to_str().unwrap_or_default()) {
        None | Some("same-origin") | Some("none") => Ok(()),
        Some(_) => {
            let fetch = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default();
            if !is_api && fetch("sec-fetch-mode") == "navigate" && fetch("sec-fetch-dest") == "document" { Ok(()) } else { Err("cross-site request refused") }
        }
    }
}

/// Refuses requests that do not come from hangar's own page (see
/// [`request_allowed`]) and adds security headers to every answer.
async fn same_origin_only(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let host = format!("127.0.0.1:{}", st.port);
    let is_api = req.uri().path().starts_with("/api/");
    let mut res = match request_allowed(req.headers(), req.uri().authority().map(|a| a.as_str()), &host, is_api) {
        Ok(()) => next.run(req).await,
        Err(why) => (StatusCode::FORBIDDEN, Json(json!({"error": format!("{why}; open hangar at http://{host} (run `hangar up`)")}))).into_response(),
    };
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    // Conversations and the token must never sit in a browser or proxy cache.
    if is_api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
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
    let manager = st.manager.clone();
    let unshared = tokio::task::spawn_blocking(move || manager.unshared_servers()).await.unwrap_or_default();
    let v = json!({
        "machine": st.manager.machine(),
        "servers": servers,
        "unshared": unshared,
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
    sys["projects"] = json!(recent_projects(&st).await);
    sys["home"] = json!(crate::util::user_home().display().to_string());
    Json(sys).into_response()
}

/// Reads Claude Code's transcripts, so off the async threads.
async fn recent_projects(st: &AppState) -> Vec<String> {
    let manager = st.manager.clone();
    tokio::task::spawn_blocking(move || manager.recent_projects()).await.unwrap_or_default()
}

#[derive(Deserialize)]
struct FoldersQuery {
    #[serde(default)]
    path: String,
}

/// Folder names for the app's folder picker (never files or contents). The
/// top level also carries the recent folders, fresh each time it opens.
async fn list_folders(State(st): State<AppState>, Query(q): Query<FoldersQuery>) -> Response {
    let top = q.path.trim().is_empty();
    let listing = match tokio::task::spawn_blocking(move || crate::folders::list(&q.path, &crate::util::user_home())).await {
        Ok(listing) => listing,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut v = serde_json::to_value(listing).unwrap_or(Value::Null);
    if top {
        v["recent"] = json!(recent_projects(&st).await);
    }
    Json(v).into_response()
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// Folders found by name near the recent folders and in home.
async fn search_folders(State(st): State<AppState>, Query(q): Query<SearchQuery>) -> Response {
    let manager = st.manager.clone();
    let found = tokio::task::spawn_blocking(move || {
        use std::path::{Path, PathBuf};
        let home = crate::util::user_home();
        let recent: Vec<PathBuf> = manager.recent_projects().into_iter().map(PathBuf::from).collect();
        let mut roots = recent.clone();
        // Their siblings are likely too, but not a whole drive or every
        // user's home.
        roots.extend(recent.iter().filter_map(|r| r.parent()).filter(|p| p.parent().is_some() && Some(*p) != home.parent()).map(Path::to_path_buf));
        roots.push(home);
        crate::folders::search(&q.q, &roots)
    })
    .await;
    match found {
        Ok(entries) => Json(json!({ "entries": entries })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct TrustQuery {
    cwd: String,
}

/// Reads files the folder controls, so off the async threads.
async fn get_trust(State(st): State<AppState>, Query(q): Query<TrustQuery>) -> Response {
    let cwd = std::path::PathBuf::from(q.cwd.trim());
    let manager = st.manager.clone();
    let info =
        tokio::task::spawn_blocking(move || if cwd.is_dir() { Ok(manager.trust(&cwd)) } else { Err(format!("{} is not a folder", cwd.display())) }).await;
    match info {
        Ok(Ok(info)) => Json(info).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
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
    trust_seen: Option<String>,
    #[serde(default)]
    create_folder: bool,
}

async fn create_session(State(st): State<AppState>, Json(b): Json<CreateBody>) -> Response {
    let req = NewSession {
        cwd: b.cwd,
        prompt: b.prompt,
        title: b.title,
        model: b.model,
        permission_mode: b.permission_mode,
        trust: b.trust,
        trust_seen: b.trust_seen,
        create_folder: b.create_folder,
    };
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
    model: Option<String>,
}

async fn update_session(State(st): State<AppState>, Path(id): Path<String>, Json(b): Json<UpdateBody>) -> Response {
    match st.manager.update(&id, crate::sessions::Update { title: b.title, permission_mode: b.permission_mode, model: b.model }) {
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

#[derive(Deserialize)]
struct SharingBody {
    on: bool,
}

/// `hangar wrap` and `hangar unwrap`, from the app. It edits the Claude
/// config, so off the async threads.
async fn set_sharing(State(st): State<AppState>, Json(body): Json<SharingBody>) -> Response {
    let manager = st.manager.clone();
    let done = tokio::task::spawn_blocking(move || manager.set_sharing(body.on)).await;
    *st.snapshot.lock().await = None; // the next snapshot shows the new state
    match done {
        Ok(Ok(changed)) => Json(json!({"ok": true, "changed": changed})).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
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
