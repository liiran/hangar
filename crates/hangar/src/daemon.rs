//! The hangar daemon: accepts shims, runs the pool and the sessions, serves
//! the UI, and answers control requests from the CLI.
//!
//! Shims and the CLI connect over loopback TCP. Every connection starts with
//! a [`Challenge`]: the daemon proves it knows the token from `daemon.json`
//! before the client reveals anything. The client then sends the token:
//!
//! * a [`Hello`]: a shim standing in for an MCP server;
//! * `{"control": ..., "token": ...}`: `status`, `stop`, `stop-server` or
//!   `pair` from the CLI.

use crate::config::Config;
use crate::jsonrpc;
use crate::policy::{self, Share};
use crate::pool::{AttachRequest, Pool, PoolOptions};
use crate::procs;
use crate::util::Locked;
use crate::util::{constant_time_eq, make_private, now_ms, random_hex, write_private};
use anyhow::{Context, Result, bail};
use hangar_proto::{Challenge, DaemonInfo, Hello, Proof, WIRE_VERSION, Welcome};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

pub struct DaemonOptions {
    pub home: PathBuf,
    pub config: Config,
    /// Serve the web UI on this port (0 = any free port). `None` = no UI.
    pub ui_port: Option<u16>,
}

/// The UI's default port; any free port is used if it is taken.
pub const DEFAULT_UI_PORT: u16 = 7317;

/// The longest first line an unauthenticated peer may send.
const MAX_CHALLENGE: u64 = 4 << 10;
/// The longest hello (it carries the shim's environment).
const MAX_HELLO: u64 = 4 << 20;
/// How long a pairing code from `hangar up` stays valid.
const PAIRING_TTL: Duration = Duration::from_secs(120);

/// One-time codes that let a browser tab obtain the UI token.
#[derive(Default)]
pub struct Pairing {
    codes: Mutex<HashMap<String, Instant>>,
}

impl Pairing {
    pub fn issue(&self) -> String {
        let code = random_hex(16);
        let mut codes = self.codes.locked();
        codes.retain(|_, t| t.elapsed() < PAIRING_TTL);
        codes.insert(code.clone(), Instant::now());
        code
    }

    /// Consumes `code`; true if it was issued and is still fresh.
    pub fn redeem(&self, code: &str) -> bool {
        let mut codes = self.codes.locked();
        let found = codes.keys().find(|k| constant_time_eq(k, code)).cloned();
        match found.and_then(|k| codes.remove(&k)) {
            Some(t) => t.elapsed() < PAIRING_TTL,
            None => false,
        }
    }
}

/// A running daemon. Dropping it does not stop it; call [`Daemon::shutdown`].
pub struct Daemon {
    pub info: DaemonInfo,
    pub pool: Arc<Pool>,
    pub sessions: Arc<crate::sessions::Manager>,
    pub pairing: Arc<Pairing>,
    pub home: PathBuf,
    ui: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: watch::Sender<bool>,
    accept: tokio::task::JoinHandle<()>,
    _lock: std::fs::File,
}

struct Shared {
    pool: Arc<Pool>,
    config: Config,
    token: String,
    info: DaemonInfo,
    pairing: Arc<Pairing>,
    shutdown_tx: watch::Sender<bool>,
}

#[derive(Debug)]
pub struct AlreadyRunning;

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another hangar daemon is already running")
    }
}

impl std::error::Error for AlreadyRunning {}

impl Daemon {
    pub async fn start(opts: DaemonOptions) -> Result<Daemon> {
        std::fs::create_dir_all(&opts.home).with_context(|| format!("creating {}", opts.home.display()))?;
        make_private(&opts.home);
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(opts.home.join("daemon.lock"))?;
        if lock.try_lock().is_err() {
            bail!(AlreadyRunning);
        }

        let token = random_hex(24);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("binding the shim port")?;
        let port = listener.local_addr()?.port();
        let ui_listener = match opts.ui_port {
            None => None,
            Some(p) => Some(bind_ui(&opts.home, p).await?),
        };
        let ui_port = match &ui_listener {
            Some(l) => Some(l.local_addr()?.port()),
            None => None,
        };
        let info = DaemonInfo {
            pid: std::process::id(),
            shim_port: port,
            ui_port,
            token: token.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            started_at_ms: now_ms(),
        };

        let p = &opts.config.pool;
        let pool = Pool::new(PoolOptions {
            idle_timeout: p.idle_timeout(),
            session_grace: p.session_grace(),
            init_timeout: p.init_timeout(),
            fast_npx: p.fast_npx,
            state_dir: Some(opts.home.clone()),
        });
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let sessions = crate::sessions::Manager::start(&opts.home, &opts.config);
        let pairing = Arc::new(Pairing::default());
        let ui = ui_listener.map(|l| {
            // The UI has its own token, which the daemon never writes to disk
            // (the page keeps it in its own storage until the daemon restarts).
            let state = crate::api::AppState::new(sessions.clone(), pool.clone(), random_hex(24), pairing.clone(), ui_port.unwrap_or(0));
            tokio::spawn(crate::api::serve(l, state, shutdown_tx.subscribe()))
        });
        let shared =
            Arc::new(Shared { pool: pool.clone(), config: opts.config, token, info: info.clone(), pairing: pairing.clone(), shutdown_tx: shutdown_tx.clone() });
        // Announce ourselves only once everything is listening.
        write_info(&opts.home, &info)?;

        let accept = tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            // Out of sockets, most likely: wait instead of spinning.
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            continue;
                        };
                        let shared = shared.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle(stream, shared).await {
                                tracing::debug!("connection ended: {e:#}");
                            }
                        });
                    }
                    _ = shutdown_rx.changed() => break,
                }
            }
        });
        tracing::info!(port, home = %opts.home.display(), "hangar daemon listening");
        Ok(Daemon { info, pool, sessions, pairing, ui, home: opts.home, shutdown_tx, accept, _lock: lock })
    }

    /// Resolves when someone asks the daemon to stop (`hangar stop`).
    pub async fn stopped(&self) {
        let mut rx = self.shutdown_tx.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.accept.await;
        if let Some(ui) = self.ui {
            let _ = tokio::time::timeout(Duration::from_secs(3), ui).await;
        }
        self.sessions.shutdown();
        self.pool.shutdown().await;
        // Only remove the discovery file if it is still ours.
        if hangar_proto::read_daemon_info(&self.home).is_some_and(|i| i.token == self.info.token) {
            let _ = std::fs::remove_file(hangar_proto::daemon_info_path(&self.home));
        }
        tracing::info!("hangar daemon stopped");
    }
}

fn write_info(home: &Path, info: &DaemonInfo) -> Result<()> {
    write_private(&hangar_proto::daemon_info_path(home), &serde_json::to_vec_pretty(info)?)?;
    Ok(())
}

async fn write_line(w: &mut tokio::net::tcp::OwnedWriteHalf, v: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    Ok(())
}

/// Reads one line of at most `limit` bytes.
async fn read_limited(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>, limit: u64, timeout: Duration) -> Result<String> {
    let mut line = String::new();
    let mut limited = (&mut *reader).take(limit);
    tokio::time::timeout(timeout, limited.read_line(&mut line)).await.context("timed out")??;
    if !line.ends_with('\n') {
        bail!("line too long or incomplete");
    }
    Ok(line)
}

/// Connections that have not shown the token yet. Each may make the daemon
/// hold a few megabytes for up to 20 s, so their number is capped.
static UNPROVEN: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(64);

/// The two fields of a handshake line that are read before anything else,
/// without building the rest of it in memory.
#[derive(serde::Deserialize)]
struct Credentials {
    #[serde(default)]
    token: String,
    #[serde(default)]
    control: Option<String>,
}

async fn handle(stream: TcpStream, shared: Arc<Shared>) -> Result<()> {
    stream.set_nodelay(true)?;
    let unproven = UNPROVEN.acquire().await?;
    let (rd, mut wr) = stream.into_split();
    // Lines longer than this are still read whole; it is just the up-front allocation.
    let mut reader = BufReader::with_capacity(16 * 1024, rd);

    // 1. Prove we hold the token before the client says anything secret.
    let first = read_limited(&mut reader, MAX_CHALLENGE, Duration::from_secs(10)).await?;
    let challenge: Challenge = serde_json::from_str(first.trim()).context("expected a challenge")?;
    if challenge.hangar != WIRE_VERSION {
        write_line(
            &mut wr,
            &Welcome { ok: false, error: Some(format!("client speaks wire version {}, daemon speaks {WIRE_VERSION}", challenge.hangar)), ..Default::default() },
        )
        .await?;
        return Ok(());
    }
    write_line(&mut wr, &Proof { proof: hangar_proto::proof(&shared.token, &challenge.challenge) }).await?;

    // 2. The client proves it holds the token too.
    let second = read_limited(&mut reader, MAX_HELLO, Duration::from_secs(10)).await?;
    // The token is checked before the line is parsed in full: whoever sent
    // it may be anyone on this machine.
    let who: Credentials = serde_json::from_str(second.trim()).context("handshake is not JSON")?;
    if !constant_time_eq(&who.token, &shared.token) {
        if who.control.is_some() {
            write_line(&mut wr, &json!({"ok": false, "error": "bad token"})).await?;
        } else {
            write_line(&mut wr, &Welcome { ok: false, error: Some("bad token".into()), ..Default::default() }).await?;
        }
        return Ok(());
    }
    drop(unproven);
    let value: Value = serde_json::from_str(second.trim()).context("handshake is not JSON")?;
    if let Some(cmd) = who.control.as_deref() {
        write_line(&mut wr, &control(cmd, &value, &shared).await).await?;
        return Ok(());
    }
    let hello: Hello = serde_json::from_value(value).context("bad hello")?;
    let decision = policy::decide(&hello.spec, &hello.name, &shared.config);
    if decision.share == Share::Off {
        write_line(&mut wr, &Welcome { ok: true, direct: true, mode: Some("off".into()), error: None }).await?;
        return Ok(());
    }
    write_line(&mut wr, &Welcome { ok: true, direct: false, mode: Some(decision.share.as_str().into()), error: None }).await?;

    // Wait for the client's initialize request (a reconnecting shim hands it
    // to us in the hello). Clients may probe first: Claude Code sends
    // `server/discover` from a newer protocol revision and falls back to
    // initialize when the server does not know it. Pooling works over the
    // initialize handshake, so answer probes the way today's servers do.
    let (init, respond_init) = match hello.resume.clone() {
        Some(v) => (v, false),
        None => loop {
            let mut line = String::new();
            let n = tokio::time::timeout(Duration::from_secs(120), reader.read_line(&mut line)).await.context("no initialize")??;
            if n == 0 {
                return Ok(());
            }
            let msg = match serde_json::from_str::<Value>(line.trim()) {
                Ok(m) => m,
                Err(_) => {
                    if !line.trim().is_empty() {
                        write_line(&mut wr, &jsonrpc::error(Value::Null, jsonrpc::PARSE_ERROR, "parse error")).await?;
                    }
                    continue;
                }
            };
            match jsonrpc::classify(&msg) {
                jsonrpc::Kind::Request { method: "initialize", .. } => break (msg, true),
                jsonrpc::Kind::Request { id, method } => {
                    let err = jsonrpc::error(id.clone(), jsonrpc::METHOD_NOT_FOUND, format!("Method not found: {method}"));
                    write_line(&mut wr, &err).await?;
                }
                _ => {} // notifications before initialize mean nothing yet
            }
        },
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });

    let req = AttachRequest {
        // The name comes from a configuration file, which a downloaded folder can supply.
        // It is shown in `hangar status`, the app and the log, so it is cleaned here, once.
        name: crate::util::printable(&hello.name),
        spec: hello.spec.clone(),
        share: decision.share,
        cwd: PathBuf::from(&hello.cwd),
        env: hello.env.clone(),
        session: hello.session.as_deref().map(crate::util::printable),
        client_pid: hello.client_pid,
        shim_pid: hello.shim_pid,
        init: init.clone(),
        respond_init,
        to_client: tx.clone(),
        http_args: decision.http_args.clone(),
    };
    let attached = match shared.pool.attach(req).await {
        Ok(a) => a,
        Err(e) => {
            if let Some(id) = init.get("id").cloned().filter(|_| respond_init) {
                let _ = tx.send(jsonrpc::to_line(&jsonrpc::error(id, jsonrpc::SERVER_UNAVAILABLE, format!("{e:#}"))));
            }
            drop(tx);
            let _ = writer.await;
            return Ok(());
        }
    };

    let mut buf = Vec::with_capacity(4 * 1024);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&buf) {
            // JSON-RPC batches were removed from MCP; handle each element.
            Ok(Value::Array(batch)) => batch.into_iter().for_each(|m| attached.send(m)),
            Ok(msg) => attached.send(msg),
            Err(_) => {
                let _ = tx.send(jsonrpc::to_line(&jsonrpc::error(Value::Null, jsonrpc::PARSE_ERROR, "parse error")));
            }
        }
    }
    drop(tx);
    drop(attached); // detaches; the upstream drops our sender and the writer ends
    let _ = writer.await;
    Ok(())
}

#[derive(Serialize)]
struct ServerStatus {
    #[serde(flatten)]
    stats: crate::pool::upstream::UpstreamStats,
    memory: procs::Usage,
}

/// What `status` may show about the daemon (never the token).
fn public_info(info: &DaemonInfo) -> Value {
    json!({"pid": info.pid, "shim_port": info.shim_port, "ui_port": info.ui_port, "version": info.version, "started_at_ms": info.started_at_ms})
}

async fn control(cmd: &str, req: &Value, shared: &Shared) -> Value {
    match cmd {
        "status" => {
            let stats = shared.pool.stats().await;
            let snap = procs::snapshot();
            let servers: Vec<ServerStatus> = stats
                .into_iter()
                .map(|s| {
                    let memory = s.pid.map(|pid| procs::Usage::of(procs::tree(&snap, pid))).unwrap_or_default();
                    ServerStatus { stats: s, memory }
                })
                .collect();
            let daemon = snap.iter().find(|p| p.pid == shared.info.pid).map(|p| procs::Usage::of([p])).unwrap_or_default();
            json!({"ok": true, "daemon": public_info(&shared.info), "daemon_memory": daemon, "servers": servers, "served": shared.pool.served()})
        }
        "stop" => {
            let _ = shared.shutdown_tx.send(true);
            json!({"ok": true})
        }
        "stop-server" => {
            let id = req.get("id").and_then(Value::as_u64).unwrap_or(0);
            json!({"ok": shared.pool.stop(id).await})
        }
        "pair" => json!({"ok": true, "code": shared.pairing.issue(), "ui_port": shared.info.ui_port}),
        other => json!({"ok": false, "error": format!("unknown control command {other}")}),
    }
}

/// Binds the app's port: the preferred one, else the one this home used last,
/// else any free one. Reusing the last port lets windows that are already
/// open find a restarted daemon when the preferred port is taken (say, by
/// another hangar).
async fn bind_ui(home: &Path, preferred: u16) -> Result<TcpListener> {
    let file = home.join("ui-port");
    let last = std::fs::read_to_string(&file).ok().and_then(|s| s.trim().parse::<u16>().ok());
    let mut listener = None;
    for p in [preferred, last.unwrap_or(0)].into_iter().filter(|p| *p != 0) {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)).await {
            listener = Some(l);
            break;
        }
    }
    let listener = match listener {
        Some(l) => l,
        None => TcpListener::bind(("127.0.0.1", 0)).await.context("binding the UI port")?,
    };
    let _ = crate::util::write_private(&file, listener.local_addr()?.port().to_string().as_bytes());
    Ok(listener)
}

/// Sends a control request to the running daemon, after checking that it
/// really is the daemon.
pub async fn send_control(home: &Path, cmd: &str, extra: Value) -> Result<Value> {
    let info = hangar_proto::read_daemon_info(home).context("hangar daemon is not running")?;
    let stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(("127.0.0.1", info.shim_port)))
        .await
        .context("hangar daemon is not answering")?
        .context("hangar daemon is not running")?;
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);
    let nonce = hangar_proto::nonce();
    write_line(&mut wr, &Challenge { hangar: WIRE_VERSION, challenge: nonce.clone() }).await?;
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await.context("no answer from the daemon")??;
    let proof: Proof = serde_json::from_str(line.trim()).context("the process on the hangar port is not the hangar daemon")?;
    if !constant_time_eq(&proof.proof, &hangar_proto::proof(&info.token, &nonce)) {
        bail!("the process on the hangar port is not the hangar daemon");
    }
    let mut req = json!({"control": cmd, "token": info.token});
    if let (Some(obj), Some(more)) = (req.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            if k != "control" && k != "token" {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    write_line(&mut wr, &req).await?;
    line.clear();
    tokio::time::timeout(Duration::from_secs(15), reader.read_line(&mut line)).await.context("no answer from the daemon")??;
    Ok(serde_json::from_str(line.trim())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_app_keeps_its_port_across_restarts_when_the_default_is_taken() {
        let home = std::env::temp_dir().join(format!("hangar-uiport-{}-{}", std::process::id(), random_hex(3)));
        std::fs::create_dir_all(&home).unwrap();
        let taken = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let preferred = taken.local_addr().unwrap().port();
        let first = bind_ui(&home, preferred).await.unwrap().local_addr().unwrap().port();
        assert_ne!(first, preferred);
        let again = bind_ui(&home, preferred).await.unwrap().local_addr().unwrap().port();
        assert_eq!(again, first, "a restart comes back on the same port");
        drop(taken);
        assert_eq!(bind_ui(&home, preferred).await.unwrap().local_addr().unwrap().port(), preferred, "the preferred port wins once free");
        let _ = std::fs::remove_dir_all(home);
    }
}
