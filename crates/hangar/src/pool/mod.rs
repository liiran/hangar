//! The MCP server pool: one running copy of each server, shared by sessions.

pub mod http;
pub mod upstream;

use crate::policy::Share;
use anyhow::{anyhow, Result};
use hangar_proto::ServerSpec;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use upstream::{Cmd, ConnInfo, Upstream, UpstreamStats};

/// Which upstream a session should use.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    /// [`ServerSpec::fingerprint`]: command, args and declared env.
    pub fingerprint: String,
    /// `global`, `project:<folder>` or `session:<conn>`.
    pub scope: String,
    /// The MCP protocol version the client asked for.
    pub protocol: String,
}

#[derive(Debug, Clone)]
pub struct PoolOptions {
    pub idle_timeout: Duration,
    pub session_grace: Duration,
    pub init_timeout: Duration,
    /// Start cached npx packages with `node` directly (see [`crate::launch`]).
    pub fast_npx: bool,
}

impl Default for PoolOptions {
    fn default() -> Self {
        PoolOptions { idle_timeout: Duration::from_secs(600), session_grace: Duration::from_secs(2), init_timeout: Duration::from_secs(90), fast_npx: true }
    }
}

pub struct AttachRequest {
    pub name: String,
    pub spec: ServerSpec,
    pub share: Share,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub session: Option<String>,
    pub client_pid: Option<u32>,
    pub shim_pid: u32,
    /// The session's `initialize` request.
    pub init: Value,
    /// Whether to answer `init` (false when a shim reconnects).
    pub respond_init: bool,
    pub to_client: mpsc::UnboundedSender<String>,
}

/// A session attached to an upstream.
pub struct Attached {
    pub conn: u64,
    pub upstream: Upstream,
}

impl Attached {
    pub fn send(&self, msg: Value) {
        self.upstream.send(Cmd::Client { conn: self.conn, msg });
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.upstream.send(Cmd::Detach { conn: self.conn });
    }
}

pub struct Pool {
    opts: PoolOptions,
    upstreams: Arc<Mutex<HashMap<PoolKey, Upstream>>>,
    next_conn: AtomicU64,
    next_upstream: AtomicU64,
    on_exit: Arc<dyn Fn(u64, &PoolKey) + Send + Sync>,
}

impl Pool {
    pub fn new(opts: PoolOptions) -> Arc<Pool> {
        let upstreams: Arc<Mutex<HashMap<PoolKey, Upstream>>> = Arc::new(Mutex::new(HashMap::new()));
        let map = Arc::downgrade(&upstreams);
        // When an upstream finishes, drop it from the map (unless replaced).
        let on_exit: Arc<dyn Fn(u64, &PoolKey) + Send + Sync> = Arc::new(move |id, key| {
            if let Some(map) = map.upgrade() {
                let mut m = map.lock().unwrap();
                if m.get(key).is_some_and(|u| u.id == id) {
                    m.remove(key);
                }
            }
        });
        Arc::new(Pool { opts, upstreams, next_conn: AtomicU64::new(1), next_upstream: AtomicU64::new(1), on_exit })
    }

    pub fn options(&self) -> &PoolOptions {
        &self.opts
    }

    pub fn key_for(&self, req: &AttachRequest, conn: u64) -> PoolKey {
        let scope = match req.share {
            Share::Global | Share::Native => "global".to_string(),
            Share::Project => format!("project:{}", normalize_dir(&req.cwd)),
            Share::Session | Share::Off => format!("session:{conn}"),
        };
        let protocol = req.init.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or_default().to_string();
        PoolKey { fingerprint: req.spec.fingerprint(), scope, protocol }
    }

    /// Attaches a session, starting the server if no suitable one is running.
    pub async fn attach(&self, req: AttachRequest) -> Result<Attached> {
        let conn = self.next_conn.fetch_add(1, Ordering::SeqCst);
        let key = self.key_for(&req, conn);
        let init_id = if req.respond_init { req.init.get("id").cloned() } else { None };
        let client_init = req.init.get("params").cloned().unwrap_or(Value::Null);
        for _ in 0..5 {
            let upstream = {
                let mut map = self.upstreams.lock().unwrap();
                match map.get(&key) {
                    Some(u) if u.is_alive() => u.clone(),
                    _ => {
                        let id = self.next_upstream.fetch_add(1, Ordering::SeqCst);
                        let native = (req.share == Share::Native)
                            .then(|| http::native_args(crate::policy::package_of(&req.spec).as_deref(), &req.spec))
                            .flatten();
                        let u = if let Some(extra_args) = native {
                            http::start(http::HttpSpawn {
                                id,
                                key: key.clone(),
                                name: req.name.clone(),
                                spec: req.spec.clone(),
                                extra_args,
                                cwd: req.cwd.clone(),
                                env: req.env.clone(),
                                opts: self.opts.clone(),
                                on_exit: Arc::downgrade(&self.on_exit),
                            })
                        } else {
                            upstream::start(upstream::Spawn {
                            id,
                            key: key.clone(),
                            name: req.name.clone(),
                            share: req.share,
                            spec: req.spec.clone(),
                            cwd: req.cwd.clone(),
                            env: req.env.clone(),
                            client_init: client_init.clone(),
                            opts: self.opts.clone(),
                            on_exit: Arc::downgrade(&self.on_exit),
                        })
                        };
                        map.insert(key.clone(), u.clone());
                        u
                    }
                }
            };
            let (reply, rx) = oneshot::channel();
            let info = ConnInfo {
                conn,
                name: req.name.clone(),
                session: req.session.clone(),
                client_pid: req.client_pid,
                shim_pid: req.shim_pid,
                tx: req.to_client.clone(),
                attached_at_ms: upstream::now_ms(),
            };
            if !upstream.send(Cmd::Attach { info, init_id: init_id.clone(), init_params: client_init.clone(), reply }) {
                self.forget(&key, upstream.id);
                continue;
            }
            match rx.await {
                Ok(Ok(())) => return Ok(Attached { conn, upstream }),
                _ => {
                    // It was shutting down; start a fresh one.
                    self.forget(&key, upstream.id);
                }
            }
        }
        Err(anyhow!("could not attach to {}", req.name))
    }

    fn forget(&self, key: &PoolKey, id: u64) {
        let mut map = self.upstreams.lock().unwrap();
        if map.get(key).is_some_and(|u| u.id == id) {
            map.remove(key);
        }
    }

    pub fn upstreams(&self) -> Vec<Upstream> {
        self.upstreams.lock().unwrap().values().cloned().collect()
    }

    pub async fn stats(&self) -> Vec<UpstreamStats> {
        let mut out = Vec::new();
        for u in self.upstreams() {
            if let Some(s) = u.stats().await {
                out.push(s);
            }
        }
        out.sort_by_key(|s| s.id);
        out
    }

    /// Stops one upstream by id. Returns false if it was not found.
    pub async fn stop(&self, id: u64) -> bool {
        let Some(u) = self.upstreams().into_iter().find(|u| u.id == id) else { return false };
        let (reply, rx) = oneshot::channel();
        if u.send(Cmd::Stop { reply: Some(reply) }) {
            let _ = rx.await;
        }
        self.forget(&u.key, u.id);
        true
    }

    /// Stops every server and waits until they are gone.
    pub async fn shutdown(&self) {
        let all: Vec<Upstream> = self.upstreams.lock().unwrap().drain().map(|(_, u)| u).collect();
        let mut waits = Vec::new();
        for u in all {
            let (reply, rx) = oneshot::channel();
            if u.send(Cmd::Stop { reply: Some(reply) }) {
                waits.push(rx);
            }
        }
        for w in waits {
            let _ = tokio::time::timeout(Duration::from_secs(5), w).await;
        }
    }
}

fn normalize_dir(p: &std::path::Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) { s.to_ascii_lowercase() } else { s }
}
