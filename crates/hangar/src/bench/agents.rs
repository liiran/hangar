//! The agents a benchmark run starts: simulated MCP clients or real
//! headless Claude Code, each talking to servers directly or through a pool.

use super::{Agent, RunConfig};
use crate::client::McpClient;
use anyhow::{Context, Result, bail};
use hangar_proto::ServerSpec;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

pub const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// A pool daemon started for one run, in its own state directory.
pub struct Pool {
    pub home: PathBuf,
    pub daemon_pid: u32,
    pub shim: PathBuf,
}

/// Starts `hangar daemon` (no UI) in a fresh home under `workdir`.
pub async fn start_pool(workdir: &Path, fast_npx: bool) -> Result<Pool> {
    let home = workdir.join(format!("home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home)?;
    if !fast_npx {
        std::fs::write(home.join("config.toml"), "[pool]\nfast_npx = false\n")?;
    }
    let exe = std::env::current_exe()?;
    let shim = exe.with_file_name(if cfg!(windows) { "hangar-shim.exe" } else { "hangar-shim" });
    if !shim.is_file() {
        bail!("hangar-shim not found next to {}", exe.display());
    }
    let mut cmd = Command::new(&exe);
    cmd.args(["daemon", "--quiet", "--no-ui"]).env(hangar_proto::ENV_HOME, &home);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    crate::procs::hide_window(&mut cmd);
    let child = cmd.spawn().context("starting the pool daemon")?;
    let pid = child.id().unwrap_or(0);
    std::mem::forget(child); // the caller's job owns it
    let deadline = Instant::now() + Duration::from_secs(15);
    while hangar_proto::read_daemon_info(&home).is_none() {
        if Instant::now() > deadline {
            bail!("the pool daemon did not start");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(Pool { home, daemon_pid: pid, shim })
}

/// The command an agent runs for one server: the server itself, or the shim.
pub fn server_command(name: &str, spec: &ServerSpec, workdir: &Path, pool: Option<&Pool>) -> Command {
    let mut cmd = match pool {
        None => {
            let mut c = Command::new(hangar_shim::resolve_command(&spec.command).unwrap_or_else(|| PathBuf::from(&spec.command)));
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
    crate::procs::hide_window(&mut cmd);
    cmd
}

/// The entry for one server in a Claude Code `--mcp-config` file.
pub fn mcp_config_entry(name: &str, spec: &ServerSpec, pool: Option<&Pool>) -> Value {
    match pool {
        None => json!({"command": spec.command, "args": spec.args, "env": spec.env}),
        Some(pool) => {
            let mut args = vec!["--name".to_string(), name.to_string(), "--".into(), spec.command.clone()];
            args.extend(spec.args.iter().cloned());
            let mut env = spec.env.clone();
            env.insert(hangar_proto::ENV_HOME.into(), pool.home.display().to_string());
            env.insert(hangar_proto::ENV_NO_AUTOSTART.into(), "1".into());
            json!({"command": pool.shim.display().to_string(), "args": args, "env": env})
        }
    }
}

// A handful per run, never copied: the size difference does not matter.
#[allow(clippy::large_enum_variant)]
pub enum Session {
    Sim { clients: Vec<McpClient>, errors: Vec<String> },
    Claude { child: tokio::process::Child, pid: u32, errors: Vec<String> },
}

impl Session {
    pub fn errors(&self) -> &[String] {
        match self {
            Session::Sim { errors, .. } | Session::Claude { errors, .. } => errors,
        }
    }

    /// The agent process, for real agents.
    pub fn agent_pid(&self) -> Option<u32> {
        match self {
            Session::Claude { pid, .. } => Some(*pid),
            Session::Sim { .. } => None,
        }
    }

    /// Ends the session the way an agent does (or crashes it, with `kill`).
    pub async fn close(self, kill: bool) {
        match self {
            Session::Sim { clients, .. } => {
                for c in &clients {
                    c.shutdown_like_agent().await;
                }
            }
            Session::Claude { mut child, .. } if kill => {
                let _ = child.start_kill();
                let _ = child.wait().await;
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

/// Starts one session; returns it and its ready time (`None` if it failed).
pub async fn launch(index: usize, cfg: &RunConfig, pool: Option<&Pool>) -> Result<(Session, Option<u64>)> {
    match cfg.agent {
        Agent::Sim => launch_sim(index, cfg, pool).await,
        Agent::Claude => launch_claude(index, cfg, pool).await,
    }
}

async fn launch_sim(index: usize, cfg: &RunConfig, pool: Option<&Pool>) -> Result<(Session, Option<u64>)> {
    let started = Instant::now();
    let mut handles = Vec::new();
    for (name, spec) in &cfg.servers {
        let cmd = server_command(name, spec, &cfg.workdir, pool);
        let root = cfg.workdir.display().to_string();
        let name = name.clone();
        let browse = cfg.browse;
        handles.push(async move {
            let client = McpClient::spawn(cmd, vec![root]).with_context(|| format!("{name}: spawn"))?;
            let setup = async {
                client.initialize("hangar-bench", READY_TIMEOUT).await?;
                let tools = client.call("tools/list", json!({}), READY_TIMEOUT).await?;
                let has = |t: &str| tools["tools"].as_array().is_some_and(|a| a.iter().any(|x| x["name"] == t));
                if browse && has("browser_navigate") {
                    browse_and_check(&client, index).await?;
                }
                anyhow::Ok(())
            };
            let err = setup.await.err().map(|e| format!("{name}: {e:#}"));
            anyhow::Ok((client, err))
        });
    }
    let mut clients = Vec::new();
    let mut errors = Vec::new();
    for outcome in join_all(handles).await {
        match outcome {
            Ok((client, err)) => {
                errors.extend(err);
                clients.push(client);
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    let ms = errors.is_empty().then(|| started.elapsed().as_millis() as u64);
    Ok((Session::Sim { clients, errors }, ms))
}

/// Opens a page titled after this session, then checks the session sees
/// exactly that page and no other session's.
async fn browse_and_check(client: &McpClient, index: usize) -> Result<()> {
    let title = format!("hangar-session-{index}");
    let url = format!("data:text/html,<title>{title}</title><h1>{title}</h1>");
    let nav = client.call("tools/call", json!({"name": "browser_navigate", "arguments": {"url": url}}), READY_TIMEOUT).await?;
    if nav["isError"] == true {
        bail!("browser_navigate failed: {}", nav["content"][0]["text"].as_str().unwrap_or("?").lines().take(3).collect::<Vec<_>>().join(" "));
    }
    let tabs = client.call("tools/call", json!({"name": "browser_tabs", "arguments": {"action": "list"}}), READY_TIMEOUT).await?;
    let text = tabs["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if !text.contains(&format!("[{title}]")) {
        bail!("session {index} does not see its own page: {text}");
    }
    let seen: std::collections::BTreeSet<usize> =
        text.split("hangar-session-").skip(1).filter_map(|rest| rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()).collect();
    if seen.len() != 1 {
        bail!("session {index} sees other sessions' pages {seen:?}: {text}");
    }
    Ok(())
}

/// Polls futures together without pulling in the `futures` crate.
pub async fn join_all<F: std::future::Future<Output = T>, T>(futs: Vec<F>) -> Vec<T> {
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

/// Real headless Claude Code. Readiness comes from its own `mcp_status`
/// control request, so no prompt is sent and no tokens are used.
async fn launch_claude(index: usize, cfg: &RunConfig, pool: Option<&Pool>) -> Result<(Session, Option<u64>)> {
    let claude = cfg.claude_bin.clone().or_else(crate::agent::find_claude).context("claude not found; pass --claude-bin")?;
    let config_path = cfg.workdir.join(format!("mcp-{}-{index}.json", cfg.mode));
    let servers: serde_json::Map<String, Value> = cfg.servers.iter().map(|(name, spec)| (name.clone(), mcp_config_entry(name, spec, pool))).collect();
    std::fs::write(&config_path, serde_json::to_vec_pretty(&json!({"mcpServers": servers}))?)?;

    let started = Instant::now();
    let mut cmd = Command::new(&claude);
    cmd.args(["--output-format", "stream-json", "--verbose", "--input-format", "stream-json", "--strict-mcp-config", "--mcp-config"])
        .arg(&config_path)
        .current_dir(&cfg.workdir)
        .env_remove("CLAUDECODE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::procs::hide_window(&mut cmd);
    let mut child = cmd.spawn().with_context(|| format!("starting {}", claude.display()))?;
    let pid = child.id().unwrap_or(0);
    let mut stdin = child.stdin.take().context("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
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
            let failed: Vec<&str> = list.iter().filter(|s| s["status"] == "failed").map(|s| s["name"].as_str().unwrap_or("?")).collect();
            if !failed.is_empty() {
                errors.push(format!("failed to connect: {}", failed.join(", ")));
                break 'poll;
            }
            if list.iter().filter(|s| s["status"] == "connected").count() >= expected {
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
    Ok((Session::Claude { child, pid, errors }, ready))
}
