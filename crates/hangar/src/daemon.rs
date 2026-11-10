//! The hangar daemon: accepts shims, runs the pool, answers control requests.
//!
//! Shims connect over loopback TCP. Every connection must present the random
//! token from `daemon.json`, which only the current user can read. The first
//! line decides what the connection is:
//!
//! * a [`Hello`]: a shim standing in for an MCP server;
//! * `{"control": ..., "token": ...}`: `status`, `stop` or `logs` from the CLI.

use crate::config::Config;
use crate::policy::{self, Share};
use crate::pool::{AttachRequest, Pool, PoolOptions};
use crate::procs;
use anyhow::{bail, Context, Result};
use hangar_proto::{DaemonInfo, Hello, Welcome, WIRE_VERSION};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

pub struct DaemonOptions {
    pub home: PathBuf,
    pub config: Config,
}

/// A running daemon. Dropping it does not stop it; call [`Daemon::shutdown`].
pub struct Daemon {
    pub info: DaemonInfo,
    pub pool: Arc<Pool>,
    pub home: PathBuf,
    shutdown_tx: watch::Sender<bool>,
    accept: tokio::task::JoinHandle<()>,
    _lock: std::fs::File,
}

struct Shared {
    pool: Arc<Pool>,
    config: Config,
    token: String,
    info: DaemonInfo,
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
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(opts.home.join("daemon.lock"))?;
        if lock.try_lock().is_err() {
            bail!(AlreadyRunning);
        }

        let token = random_token();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("binding the shim port")?;
        let port = listener.local_addr()?.port();
        let info = DaemonInfo {
            pid: std::process::id(),
            shim_port: port,
            ui_port: None,
            token: token.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            started_at_ms: crate::pool::upstream::now_ms(),
        };
        write_info(&opts.home, &info)?;

        let p = &opts.config.pool;
        let pool = Pool::new(PoolOptions { idle_timeout: p.idle_timeout(), session_grace: p.session_grace(), init_timeout: p.init_timeout(), fast_npx: p.fast_npx });
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let shared = Arc::new(Shared { pool: pool.clone(), config: opts.config, token, info: info.clone(), shutdown_tx: shutdown_tx.clone() });

        let accept = tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
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
        Ok(Daemon { info, pool, home: opts.home, shutdown_tx, accept, _lock: lock })
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
        self.pool.shutdown().await;
        // Only remove the discovery file if it is still ours.
        if hangar_proto::read_daemon_info(&self.home).is_some_and(|i| i.token == self.info.token) {
            let _ = std::fs::remove_file(hangar_proto::daemon_info_path(&self.home));
        }
        tracing::info!("hangar daemon stopped");
    }
}

fn write_info(home: &Path, info: &DaemonInfo) -> Result<()> {
    let path = hangar_proto::daemon_info_path(home);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(info)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("OS randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn write_line(w: &mut tokio::net::tcp::OwnedWriteHalf, v: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    Ok(())
}

async fn handle(stream: TcpStream, shared: Arc<Shared>) -> Result<()> {
    stream.set_nodelay(true)?;
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::with_capacity(256 * 1024, rd);
    let mut first = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut first)).await.context("handshake timed out")??;
    let value: Value = serde_json::from_str(first.trim()).context("handshake is not JSON")?;

    if let Some(cmd) = value.get("control").and_then(Value::as_str) {
        if value.get("token").and_then(Value::as_str) != Some(shared.token.as_str()) {
            write_line(&mut wr, &json!({"ok": false, "error": "bad token"})).await?;
            return Ok(());
        }
        let answer = control(cmd, &value, &shared).await;
        write_line(&mut wr, &answer).await?;
        return Ok(());
    }

    let hello: Hello = serde_json::from_value(value).context("bad hello")?;
    if hello.hangar != WIRE_VERSION {
        let w = Welcome { ok: false, error: Some(format!("shim speaks wire version {}, daemon speaks {WIRE_VERSION}", hello.hangar)), ..Default::default() };
        write_line(&mut wr, &w).await?;
        return Ok(());
    }
    if hello.token != shared.token {
        write_line(&mut wr, &Welcome { ok: false, error: Some("bad token".into()), ..Default::default() }).await?;
        return Ok(());
    }
    let decision = policy::decide(&hello.spec, &hello.name, &shared.config);
    if decision.share == Share::Off {
        write_line(&mut wr, &Welcome { ok: true, direct: true, mode: Some("off".into()), error: None }).await?;
        return Ok(());
    }
    write_line(&mut wr, &Welcome { ok: true, direct: false, mode: Some(decision.share.as_str().into()), error: None }).await?;

    // The first message must be the client's initialize request, unless the
    // shim is reconnecting and hands it to us in the hello.
    let (init, respond_init) = match hello.resume.clone() {
        Some(v) => (v, false),
        None => {
            let mut line = String::new();
            let n = tokio::time::timeout(Duration::from_secs(120), reader.read_line(&mut line)).await.context("no initialize")??;
            if n == 0 {
                return Ok(());
            }
            (serde_json::from_str::<Value>(line.trim()).context("initialize is not JSON")?, true)
        }
    };
    if init.get("method").and_then(Value::as_str) != Some("initialize") {
        let id = init.get("id").cloned().unwrap_or(Value::Null);
        write_line(&mut wr, &crate::jsonrpc::error(id, crate::jsonrpc::INVALID_REQUEST, "the first message must be initialize")).await?;
        return Ok(());
    }

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
        name: hello.name.clone(),
        spec: hello.spec.clone(),
        share: decision.share,
        cwd: PathBuf::from(&hello.cwd),
        env: hello.env.clone(),
        session: hello.session.clone(),
        client_pid: hello.client_pid,
        shim_pid: hello.shim_pid,
        init: init.clone(),
        respond_init,
        to_client: tx.clone(),
    };
    let attached = match shared.pool.attach(req).await {
        Ok(a) => a,
        Err(e) => {
            if let Some(id) = init.get("id").cloned() {
                let _ = tx.send(crate::jsonrpc::to_line(&crate::jsonrpc::error(id, crate::jsonrpc::SERVER_UNAVAILABLE, format!("{e:#}"))));
            }
            drop(tx);
            let _ = writer.await;
            return Ok(());
        }
    };
    drop(tx);

    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        match serde_json::from_slice::<Value>(&buf) {
            Ok(Value::Array(batch)) => {
                for msg in batch {
                    attached.send(msg);
                }
            }
            Ok(msg) => attached.send(msg),
            Err(_) => {
                if !buf.iter().all(u8::is_ascii_whitespace) {
                    tracing::debug!("ignoring a line that is not JSON");
                }
            }
        }
    }
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
            json!({"ok": true, "daemon": shared.info.clone_without_token(), "daemon_memory": daemon, "servers": servers})
        }
        "stop" => {
            let _ = shared.shutdown_tx.send(true);
            json!({"ok": true})
        }
        "stop-server" => {
            let id = req.get("id").and_then(Value::as_u64).unwrap_or(0);
            json!({"ok": shared.pool.stop(id).await})
        }
        other => json!({"ok": false, "error": format!("unknown control command {other}")}),
    }
}

trait WithoutToken {
    fn clone_without_token(&self) -> Value;
}

impl WithoutToken for DaemonInfo {
    fn clone_without_token(&self) -> Value {
        json!({"pid": self.pid, "shim_port": self.shim_port, "ui_port": self.ui_port, "version": self.version, "started_at_ms": self.started_at_ms})
    }
}

/// Sends a control request to the running daemon.
pub async fn send_control(home: &Path, cmd: &str, extra: Value) -> Result<Value> {
    let info = hangar_proto::read_daemon_info(home).context("hangar daemon is not running")?;
    let stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(("127.0.0.1", info.shim_port)))
        .await
        .context("hangar daemon is not answering")?
        .context("hangar daemon is not running")?;
    let (rd, mut wr) = stream.into_split();
    let mut req = json!({"control": cmd, "token": info.token});
    if let (Some(obj), Some(more)) = (req.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            if k != "control" && k != "token" {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    write_line(&mut wr, &req).await?;
    let mut line = String::new();
    BufReader::new(rd).read_line(&mut line).await?;
    Ok(serde_json::from_str(line.trim())?)
}
