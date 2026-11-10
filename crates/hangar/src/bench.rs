//! `hangar bench`: what N agent sessions cost with their MCP servers started
//! the usual way (one copy per session) versus through the hangar pool.
//!
//! Each measured run happens in its own child process (`hangar bench-run`)
//! that puts itself in a fresh job object, so CPU totals include every
//! process that ran, even ones that already exited (npx spawns several).
//!
//! Two kinds of agent are supported:
//! * `sim`: a minimal MCP client per server per session, doing what an agent
//!   does at startup (initialize, initialized, tools/list). Cheap and exact.
//! * `claude`: real headless Claude Code sessions. Readiness comes from the
//!   CLI's `mcp_status` control request, so no prompt is sent and no tokens
//!   are used.

use crate::client::McpClient;
use crate::procs::{self, Job, ProcInfo, Usage};
use anyhow::{bail, Context, Result};
use hangar_proto::ServerSpec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Every session starts its own copy of every server (today's behavior).
    Direct,
    /// Sessions start `hangar-shim`, which shares servers through the daemon.
    Pooled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Sim,
    Claude,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mode::Direct => "direct",
            Mode::Pooled => "pooled",
        })
    }
}

impl std::fmt::Display for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Agent::Sim => "sim",
            Agent::Claude => "claude",
        })
    }
}

/// Servers to start, by name.
pub type ServerSet = Vec<(String, ServerSpec)>;

fn npx(args: &[&str]) -> ServerSpec {
    let mut full: Vec<String> = Vec::new();
    let command = if cfg!(windows) {
        // Claude Code on Windows requires `cmd /c` in front of npx.
        full.extend(["/c".into(), "npx".into()]);
        "cmd".to_string()
    } else {
        "npx".to_string()
    };
    full.push("-y".into());
    full.extend(args.iter().map(|s| s.to_string()));
    ServerSpec { command, args: full, env: BTreeMap::new() }
}

/// Built-in server sets.
///
/// `popular` is five widely used servers that need no API keys. `light` is
/// two of them, for quick runs. A path to a JSON file with an `mcpServers`
/// object (Claude Code's format) is also accepted.
pub fn preset(name: &str, workdir: &Path) -> Result<ServerSet> {
    let fs_root = workdir.display().to_string();
    let set = match name {
        "popular" => vec![
            ("playwright".into(), npx(&["@playwright/mcp@latest", "--headless"])),
            ("context7".into(), npx(&["@upstash/context7-mcp@latest"])),
            ("memory".into(), npx(&["@modelcontextprotocol/server-memory"])),
            ("sequential-thinking".into(), npx(&["@modelcontextprotocol/server-sequential-thinking"])),
            ("filesystem".into(), npx(&["@modelcontextprotocol/server-filesystem", &fs_root])),
        ],
        "light" => vec![
            ("context7".into(), npx(&["@upstash/context7-mcp@latest"])),
            ("memory".into(), npx(&["@modelcontextprotocol/server-memory"])),
        ],
        path => {
            let text = std::fs::read_to_string(path).with_context(|| format!("unknown preset or unreadable file: {path}"))?;
            let v: Value = serde_json::from_str(&text)?;
            let servers = v.get("mcpServers").and_then(Value::as_object).context("file has no mcpServers object")?;
            let mut set = Vec::new();
            for (name, s) in servers {
                let Some(command) = s.get("command").and_then(Value::as_str) else { continue };
                let spec = ServerSpec {
                    command: command.to_string(),
                    args: s.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                    env: s
                        .get("env")
                        .and_then(Value::as_object)
                        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect())
                        .unwrap_or_default(),
                };
                set.push((name.clone(), spec));
            }
            set
        }
    };
    if set.is_empty() {
        bail!("preset {name} has no stdio servers");
    }
    Ok(set)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub mode: Mode,
    pub agent: Agent,
    pub sessions: usize,
    pub servers: Vec<String>,
    /// Time from launching a session until every one of its servers answered
    /// `tools/list` (sim) or reported `connected` (claude). `None` = failed.
    pub session_ready_ms: Vec<Option<u64>>,
    pub failures: Vec<String>,
    /// MCP-related processes: servers, their launchers, shims and the daemon.
    pub mcp: Usage,
    /// Agent processes (claude.exe); zero for the simulated agent.
    pub agents: Usage,
    pub breakdown: BTreeMap<String, Usage>,
    /// CPU used by every process in the run, from launch until the snapshot.
    pub cpu_ms: u64,
    /// Processes still alive 5 s after every session closed, not counting a
    /// pool daemon and the servers it keeps on purpose.
    pub leftovers: Usage,
    pub leftover_names: Vec<String>,
    /// Pooled only: ready times for a second wave of sessions that attach to
    /// servers that are already running.
    #[serde(default)]
    pub warm_ready_ms: Vec<Option<u64>>,
    pub wall_ms: u64,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub mode: Mode,
    pub agent: Agent,
    pub sessions: usize,
    pub servers: ServerSet,
    pub workdir: PathBuf,
    pub settle: Duration,
    pub warm_wave: bool,
    pub claude_bin: Option<PathBuf>,
}

const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// One measured run. Must be called in a process of its own (see module docs).
pub async fn run_once(cfg: RunConfig) -> Result<RunResult> {
    let job = Job::new_kill_on_close()?;
    job.assign_self().context("joining the measurement job")?;
    let me = std::process::id();
    let started = Instant::now();
    let cfg = Arc::new(cfg);

    let pool = match cfg.mode {
        Mode::Pooled => Some(start_pool(&cfg.workdir).await?),
        Mode::Direct => None,
    };
    let pool = Arc::new(pool);

    // Launch every session at once: the "many sessions start together" case.
    let mut set = tokio::task::JoinSet::new();
    for i in 0..cfg.sessions {
        let cfg = cfg.clone();
        let pool = pool.clone();
        set.spawn(async move { (i, launch_session(i, &cfg, pool.as_ref().as_ref()).await) });
    }
    let mut sessions: Vec<Option<Session>> = (0..cfg.sessions).map(|_| None).collect();
    let mut session_ready_ms = vec![None; cfg.sessions];
    let mut failures = Vec::new();
    while let Some(joined) = set.join_next().await {
        let (i, outcome) = joined?;
        match outcome {
            Ok((session, ms)) => {
                session_ready_ms[i] = ms;
                if ms.is_none() {
                    failures.extend(session.errors().iter().cloned());
                }
                sessions[i] = Some(session);
            }
            Err(e) => failures.push(format!("session {i}: {e:#}")),
        }
    }

    tokio::time::sleep(cfg.settle).await;

    // Price everything in the job except this process.
    let snap = procs::snapshot();
    let in_job: HashSet<u32> = job.pids().into_iter().collect();
    let mine = snap.iter().find(|p| p.pid == me).map(|p| p.cpu_ms).unwrap_or(0);
    let cpu_ms = job.cpu_ms().saturating_sub(mine);
    let daemon_pid = pool.as_ref().as_ref().map(|p| p.daemon_pid);
    let members: Vec<&ProcInfo> = snap.iter().filter(|p| in_job.contains(&p.pid) && p.pid != me).collect();
    let mut breakdown: BTreeMap<String, Usage> = BTreeMap::new();
    let mut mcp = Vec::new();
    let mut agents = Vec::new();
    for p in &members {
        let lower = p.name.to_ascii_lowercase();
        let kind = if lower.starts_with("claude") {
            "agent"
        } else if lower.starts_with("hangar-shim") {
            "shim"
        } else if Some(p.pid) == daemon_pid {
            "daemon"
        } else {
            "server"
        };
        let entry = breakdown.entry(kind.to_string()).or_default();
        entry.processes += 1;
        entry.private_bytes += p.private_bytes;
        entry.working_set += p.working_set;
        entry.cpu_ms += p.cpu_ms;
        if kind == "agent" { agents.push(*p) } else { mcp.push(*p) }
    }

    // Second wave: new sessions attaching to servers that are already warm.
    let mut warm_ready_ms = Vec::new();
    let mut warm_sessions = Vec::new();
    if cfg.warm_wave && cfg.mode == Mode::Pooled {
        let mut set = tokio::task::JoinSet::new();
        for i in 0..cfg.sessions {
            let cfg = cfg.clone();
            let pool = pool.clone();
            set.spawn(async move { launch_session(1000 + i, &cfg, pool.as_ref().as_ref()).await });
        }
        while let Some(joined) = set.join_next().await {
            match joined? {
                Ok((s, ms)) => {
                    warm_ready_ms.push(ms);
                    warm_sessions.push(s);
                }
                Err(_) => warm_ready_ms.push(None),
            }
        }
    }

    // End every session the way an agent does, then look for survivors.
    for s in sessions.into_iter().flatten().chain(warm_sessions) {
        s.close().await;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let snap = procs::snapshot();
    let managed: HashSet<u32> = match daemon_pid {
        Some(d) => procs::tree(&snap, d).iter().map(|p| p.pid).collect(),
        None => HashSet::new(),
    };
    let in_job: HashSet<u32> = job.pids().into_iter().collect();
    let survivors: Vec<&ProcInfo> = snap.iter().filter(|p| in_job.contains(&p.pid) && p.pid != me && !managed.contains(&p.pid)).collect();

    let result = RunResult {
        mode: cfg.mode,
        agent: cfg.agent,
        sessions: cfg.sessions,
        servers: cfg.servers.iter().map(|(n, _)| n.clone()).collect(),
        session_ready_ms,
        failures,
        mcp: Usage::of(mcp),
        agents: Usage::of(agents),
        breakdown,
        cpu_ms,
        leftovers: Usage::of(survivors.iter().copied()),
        leftover_names: survivors.iter().map(|p| p.name.clone()).collect(),
        warm_ready_ms,
        wall_ms: started.elapsed().as_millis() as u64,
    };
    // Clean up everything else in the job. (Terminating the job itself would
    // also kill this process before it reports.)
    for pid in job.pids() {
        if pid != me {
            procs::kill_tree(pid);
        }
    }
    // Keep the job open until this process exits: closing a kill-on-close job
    // we are a member of would end this process before it reports.
    std::mem::forget(job);
    Ok(result)
}

struct Pool {
    home: PathBuf,
    daemon_pid: u32,
    shim: PathBuf,
}

async fn start_pool(workdir: &Path) -> Result<Pool> {
    let home = workdir.join(format!("home-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    let exe = std::env::current_exe()?;
    let shim = exe.with_file_name(if cfg!(windows) { "hangar-shim.exe" } else { "hangar-shim" });
    if !shim.is_file() {
        bail!("hangar-shim not found next to {}", exe.display());
    }
    let mut cmd = Command::new(&exe);
    cmd.arg("daemon").arg("--quiet").env(hangar_proto::ENV_HOME, &home).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let child = cmd.spawn().context("starting the pool daemon")?;
    let pid = child.id().unwrap_or(0);
    std::mem::forget(child); // the job owns it
    let deadline = Instant::now() + Duration::from_secs(15);
    while hangar_proto::read_daemon_info(&home).is_none() {
        if Instant::now() > deadline {
            bail!("daemon did not start");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(Pool { home, daemon_pid: pid, shim })
}

enum Session {
    Sim { clients: Vec<McpClient>, errors: Vec<String> },
    Claude { child: tokio::process::Child, errors: Vec<String> },
}

impl Session {
    fn errors(&self) -> &Vec<String> {
        match self {
            Session::Sim { errors, .. } | Session::Claude { errors, .. } => errors,
        }
    }

    async fn close(self) {
        match self {
            Session::Sim { clients, .. } => {
                for c in &clients {
                    c.shutdown_like_agent().await;
                }
            }
            Session::Claude { mut child, .. } => {
                drop(child.stdin.take());
                if tokio::time::timeout(Duration::from_secs(10), child.wait()).await.is_err() {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
        }
    }
}

/// Builds the command an agent would run for one server.
fn server_command(name: &str, spec: &ServerSpec, workdir: &Path, pool: Option<&Pool>) -> Command {
    let mut cmd = match pool {
        None => {
            let program = hangar_shim::resolve_command(&spec.command).unwrap_or_else(|| PathBuf::from(&spec.command));
            let mut c = Command::new(program);
            c.args(&spec.args);
            c
        }
        Some(pool) => {
            let mut c = Command::new(&pool.shim);
            c.arg("--name").arg(name).arg("--").arg(&spec.command).args(&spec.args);
            c.env(hangar_proto::ENV_HOME, &pool.home).env(hangar_proto::ENV_NO_AUTOSTART, "1");
            c
        }
    };
    cmd.envs(&spec.env).current_dir(workdir).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

async fn launch_session(index: usize, cfg: &RunConfig, pool: Option<&Pool>) -> Result<(Session, Option<u64>)> {
    match cfg.agent {
        Agent::Sim => launch_sim(index, cfg, pool).await,
        Agent::Claude => launch_claude(index, cfg, pool).await,
    }
}

async fn launch_sim(_index: usize, cfg: &RunConfig, pool: Option<&Pool>) -> Result<(Session, Option<u64>)> {
    let started = Instant::now();
    let mut handles = Vec::new();
    for (name, spec) in &cfg.servers {
        let cmd = server_command(name, spec, &cfg.workdir, pool);
        let root = cfg.workdir.display().to_string();
        let name = name.clone();
        handles.push(async move {
            let client = McpClient::spawn(cmd, vec![root]).with_context(|| format!("{name}: spawn"))?;
            let setup = async {
                client.initialize("hangar-bench", READY_TIMEOUT).await?;
                client.call("tools/list", json!({}), READY_TIMEOUT).await?;
                anyhow::Ok(())
            };
            match setup.await {
                Ok(()) => anyhow::Ok((client, None)),
                Err(e) => Ok((client, Some(format!("{name}: {e:#}")))),
            }
        });
    }
    let mut clients = Vec::new();
    let mut errors = Vec::new();
    for outcome in futures_join(handles).await {
        match outcome {
            Ok((client, err)) => {
                if let Some(e) = err {
                    errors.push(e);
                }
                clients.push(client);
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    let ms = if errors.is_empty() { Some(started.elapsed().as_millis() as u64) } else { None };
    Ok((Session::Sim { clients, errors }, ms))
}

/// Joins futures concurrently without pulling in the `futures` crate.
async fn futures_join<F, T>(futs: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T>,
{
    let mut pinned: Vec<std::pin::Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<T>> = (0..pinned.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in pinned.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => out[i] = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending { std::task::Poll::Pending } else { std::task::Poll::Ready(()) }
    })
    .await;
    out.into_iter().map(|v| v.expect("all futures completed")).collect()
}

async fn launch_claude(index: usize, cfg: &RunConfig, pool: Option<&Pool>) -> Result<(Session, Option<u64>)> {
    let claude = cfg.claude_bin.clone().or_else(crate::agent::find_claude).context("claude not found; pass --claude-bin")?;
    let config_path = cfg.workdir.join(format!("mcp-{}-{index}.json", cfg.mode));
    let mut servers = serde_json::Map::new();
    for (name, spec) in &cfg.servers {
        let entry = match pool {
            None => json!({"command": spec.command, "args": spec.args, "env": spec.env}),
            Some(pool) => {
                let mut args = vec!["--name".to_string(), name.clone(), "--".into(), spec.command.clone()];
                args.extend(spec.args.iter().cloned());
                let mut env = spec.env.clone();
                env.insert(hangar_proto::ENV_HOME.into(), pool.home.display().to_string());
                env.insert(hangar_proto::ENV_NO_AUTOSTART.into(), "1".into());
                json!({"command": pool.shim.display().to_string(), "args": args, "env": env})
            }
        };
        servers.insert(name.clone(), entry);
    }
    std::fs::write(&config_path, serde_json::to_vec_pretty(&json!({"mcpServers": servers}))?)?;

    let started = Instant::now();
    let mut cmd = Command::new(&claude);
    cmd.args(["--output-format", "stream-json", "--verbose", "--input-format", "stream-json", "--strict-mcp-config", "--mcp-config"])
        .arg(&config_path)
        .current_dir(&cfg.workdir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().with_context(|| format!("starting {}", claude.display()))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"init\",\"request\":{\"subtype\":\"initialize\",\"hooks\":null}}\n").await?;

    let expected = cfg.servers.len();
    let mut errors = Vec::new();
    let mut ready = None;
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut seq = 0u64;
    'poll: while Instant::now() < deadline {
        seq += 1;
        let req = format!("{{\"type\":\"control_request\",\"request_id\":\"st{seq}\",\"request\":{{\"subtype\":\"mcp_status\"}}}}\n");
        if stdin.write_all(req.as_bytes()).await.is_err() {
            errors.push("claude exited".into());
            break;
        }
        // Read until the answer to this status request arrives.
        loop {
            let line = match tokio::time::timeout(Duration::from_secs(30), lines.next_line()).await {
                Ok(Ok(Some(l))) => l,
                _ => {
                    errors.push("no answer from claude".into());
                    break 'poll;
                }
            };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let resp = &msg["response"];
            if msg["type"] != "control_response" || resp["request_id"] != format!("st{seq}") {
                continue;
            }
            let list = resp["response"]["mcpServers"].as_array().cloned().unwrap_or_default();
            let connected = list.iter().filter(|s| s["status"] == "connected").count();
            let failed: Vec<String> = list.iter().filter(|s| s["status"] == "failed").map(|s| s["name"].as_str().unwrap_or("?").to_string()).collect();
            if !failed.is_empty() {
                errors.push(format!("failed to connect: {}", failed.join(", ")));
                break 'poll;
            }
            if connected >= expected {
                ready = Some(started.elapsed().as_millis() as u64);
                break 'poll;
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if ready.is_none() && errors.is_empty() {
        errors.push("timed out".into());
    }
    child.stdin = Some(stdin);
    // Keep draining stdout so the CLI never blocks on a full pipe.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Ok((Session::Claude { child, errors }, ready))
}

// ---------------------------------------------------------------- orchestration

#[derive(Debug, Clone)]
pub struct BenchOpts {
    pub sessions: Vec<usize>,
    pub preset: String,
    pub modes: Vec<Mode>,
    pub agent: Agent,
    pub settle: Duration,
    pub warm_wave: bool,
    pub warmup: bool,
    pub reserve_bytes: u64,
    pub out: Option<PathBuf>,
    pub claude_bin: Option<PathBuf>,
}

/// Runs every (sessions, mode) combination, each in a fresh child process.
pub async fn run(opts: BenchOpts) -> Result<Vec<RunResult>> {
    let workdir = std::env::temp_dir().join("hangar-bench");
    std::fs::create_dir_all(&workdir)?;
    let servers = preset(&opts.preset, &workdir)?;
    println!("hangar bench: {} {} servers ({}), agent={}, sessions={:?}", servers.len(), opts.preset, servers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "), opts.agent, opts.sessions);

    if opts.warmup {
        println!("warming the package cache (one direct start per server)...");
        warmup(&servers, &workdir).await;
    }

    let exe = std::env::current_exe()?;
    let mut results: Vec<RunResult> = Vec::new();
    for &n in &opts.sessions {
        for &mode in &opts.modes {
            if let Some(per_session) = per_session_cost(&results, mode, opts.agent) {
                let need = per_session * n as u64;
                let (_, avail) = procs::system_memory();
                if need + opts.reserve_bytes > avail {
                    println!(
                        "  skip {mode:>6} x{n:<3} needs ~{} but only {} is free (keeping {} in reserve)",
                        fmt_bytes(need),
                        fmt_bytes(avail),
                        fmt_bytes(opts.reserve_bytes)
                    );
                    continue;
                }
            }
            let out = workdir.join(format!("run-{}-{mode}-{n}.json", opts.agent));
            let _ = std::fs::remove_file(&out);
            let mut cmd = Command::new(&exe);
            cmd.arg("bench-run")
                .args(["--mode", &mode.to_string(), "--agent", &opts.agent.to_string(), "--sessions", &n.to_string(), "--preset", &opts.preset])
                .arg("--workdir")
                .arg(&workdir)
                .arg("--json")
                .arg(&out)
                .args(["--settle-ms", &opts.settle.as_millis().to_string()]);
            if opts.warm_wave {
                cmd.arg("--warm-wave");
            }
            if let Some(c) = &opts.claude_bin {
                cmd.arg("--claude-bin").arg(c);
            }
            let status = cmd.status().await?;
            let text = std::fs::read_to_string(&out).with_context(|| format!("run {mode} x{n} produced no result (exit {status})"))?;
            let r: RunResult = serde_json::from_str(&text)?;
            println!("  {}", summary_line(&r));
            results.push(r);
        }
    }

    let report = render_markdown(&results, &opts, &servers);
    println!("\n{report}");
    if let Some(out) = &opts.out {
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(out.with_extension("json"), serde_json::to_vec_pretty(&results)?)?;
        std::fs::write(out.with_extension("md"), &report)?;
        println!("wrote {} and .md", out.with_extension("json").display());
    }
    Ok(results)
}

fn per_session_cost(results: &[RunResult], mode: Mode, agent: Agent) -> Option<u64> {
    results
        .iter()
        .filter(|r| r.mode == mode && r.agent == agent && r.sessions > 0)
        .map(|r| (r.mcp.private_bytes + r.agents.private_bytes) / r.sessions as u64)
        .max()
}

async fn warmup(servers: &ServerSet, workdir: &Path) {
    let mut tasks = Vec::new();
    for (name, spec) in servers {
        let cmd = server_command(name, spec, workdir, None);
        let name = name.clone();
        let root = workdir.display().to_string();
        tasks.push(async move {
            let Ok(client) = McpClient::spawn(cmd, vec![root]) else { return (name, false) };
            let ok = client.initialize("hangar-warmup", READY_TIMEOUT).await.is_ok();
            client.shutdown_like_agent().await;
            (name, ok)
        });
    }
    for (name, ok) in futures_join(tasks).await {
        if !ok {
            println!("  warning: {name} did not start during warmup");
        }
    }
    // Give stragglers a moment to exit so they do not pollute the first run.
    tokio::time::sleep(Duration::from_secs(2)).await;
}

pub fn fmt_bytes(b: u64) -> String {
    let mb = b as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 { format!("{:.2} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

fn percentile(values: &[u64], p: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let rank = (p / 100.0 * (v.len() - 1) as f64).round() as usize;
    Some(v[rank.min(v.len() - 1)])
}

fn ready(values: &[Option<u64>]) -> Vec<u64> {
    values.iter().flatten().copied().collect()
}

fn fmt_ms(v: Option<u64>) -> String {
    match v {
        Some(ms) if ms >= 1000 => format!("{:.1} s", ms as f64 / 1000.0),
        Some(ms) => format!("{ms} ms"),
        None => "-".into(),
    }
}

fn summary_line(r: &RunResult) -> String {
    let rd = ready(&r.session_ready_ms);
    format!(
        "{:>6} x{:<3} mcp {:>9} in {:>3} procs | cpu {:>6.1} s | ready p50 {:>7} p95 {:>7} | failed {} | leftovers {}",
        r.mode.to_string(),
        r.sessions,
        fmt_bytes(r.mcp.private_bytes),
        r.mcp.processes,
        r.cpu_ms as f64 / 1000.0,
        fmt_ms(percentile(&rd, 50.0)),
        fmt_ms(percentile(&rd, 95.0)),
        r.session_ready_ms.iter().filter(|x| x.is_none()).count(),
        r.leftovers.processes
    )
}

pub fn render_markdown(results: &[RunResult], opts: &BenchOpts, servers: &ServerSet) -> String {
    let mut s = String::new();
    let (total, _) = procs::system_memory();
    s.push_str(&format!(
        "## hangar bench: {} agent, preset `{}`\n\nServers: {}. Machine: {} RAM, {} logical CPUs, {}.\n\n",
        opts.agent,
        opts.preset,
        servers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
        fmt_bytes(total),
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        std::env::consts::OS
    ));
    s.push_str("| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |\n");
    s.push_str("|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in results {
        let rd = ready(&r.session_ready_ms);
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.1} s | {} | {} | {} | {} | {} ({}) |\n",
            r.sessions,
            r.mode,
            r.mcp.processes,
            fmt_bytes(r.mcp.private_bytes),
            if r.agents.processes > 0 { fmt_bytes(r.agents.private_bytes) } else { "-".into() },
            r.cpu_ms as f64 / 1000.0,
            fmt_ms(percentile(&rd, 50.0)),
            fmt_ms(percentile(&rd, 95.0)),
            fmt_ms(rd.iter().max().copied()),
            r.session_ready_ms.iter().filter(|x| x.is_none()).count(),
            r.leftovers.processes,
            fmt_bytes(r.leftovers.private_bytes),
        ));
    }
    // Pairwise comparison for each session count that has both modes.
    let mut lines = Vec::new();
    for n in results.iter().map(|r| r.sessions).collect::<std::collections::BTreeSet<_>>() {
        let d = results.iter().find(|r| r.sessions == n && r.mode == Mode::Direct);
        let p = results.iter().find(|r| r.sessions == n && r.mode == Mode::Pooled);
        if let (Some(d), Some(p)) = (d, p) {
            let pct = |a: u64, b: u64| if a == 0 { 0.0 } else { 100.0 * (a as f64 - b as f64) / a as f64 };
            let dp95 = percentile(&ready(&d.session_ready_ms), 95.0).unwrap_or(0);
            let pp95 = percentile(&ready(&p.session_ready_ms), 95.0).unwrap_or(0);
            let mut line = format!(
                "- **{n} sessions:** MCP memory {} → {} (**−{:.0}%**), processes {} → {}, startup CPU {:.1} s → {:.1} s (**−{:.0}%**), ready p95 {} → {}",
                fmt_bytes(d.mcp.private_bytes),
                fmt_bytes(p.mcp.private_bytes),
                pct(d.mcp.private_bytes, p.mcp.private_bytes),
                d.mcp.processes,
                p.mcp.processes,
                d.cpu_ms as f64 / 1000.0,
                p.cpu_ms as f64 / 1000.0,
                pct(d.cpu_ms, p.cpu_ms),
                fmt_ms(Some(dp95)),
                fmt_ms(Some(pp95)),
            );
            if d.agents.processes > 0 {
                let dt = d.mcp.private_bytes + d.agents.private_bytes;
                let pt = p.mcp.private_bytes + p.agents.private_bytes;
                line.push_str(&format!(", whole session footprint {} → {} (−{:.0}%)", fmt_bytes(dt), fmt_bytes(pt), pct(dt, pt)));
            }
            if !p.warm_ready_ms.is_empty() {
                let w = ready(&p.warm_ready_ms);
                line.push_str(&format!(", joining warm servers p50 {} / p95 {}", fmt_ms(percentile(&w, 50.0)), fmt_ms(percentile(&w, 95.0))));
            }
            lines.push(line);
        }
    }
    if !lines.is_empty() {
        s.push_str("\n");
        s.push_str(&lines.join("\n"));
        s.push('\n');
    }
    s.push_str("\nMemory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.\n");
    s
}
