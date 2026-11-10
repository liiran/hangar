//! `hangar bench`: what N agent sessions cost with their MCP servers started
//! the usual way (one copy per session) versus through the hangar pool.
//!
//! Each measured run happens in its own child process (`hangar bench-run`)
//! that puts itself in a fresh job object, so CPU totals include every
//! process that ran, even ones that already exited (npx spawns several).
//! That accounting needs Windows job objects, so runs refuse other systems
//! rather than report numbers that miss processes.
//!
//! Two kinds of agent are supported:
//! * `sim`: a minimal MCP client per server per session, doing what an agent
//!   does at startup (initialize, initialized, tools/list). Cheap and exact.
//! * `claude`: real headless Claude Code sessions. Readiness comes from the
//!   CLI's `mcp_status` control request, so no prompt is sent and no tokens
//!   are used.

mod agents;
mod presets;
mod report;

pub use agents::{Pool, READY_TIMEOUT, mcp_config_entry, server_command, start_pool};
pub use presets::{ServerSet, npx_spec, preset};
pub use report::{percentile, render_markdown};

use crate::client::McpClient;
use crate::procs::{self, Job, ProcInfo, Usage};
use crate::util::fmt_bytes;
use agents::Session;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    /// pool daemon and the shared servers it keeps warm on purpose.
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
    /// Sim agent: every session opens a page in its browser and checks that
    /// it sees only its own page.
    pub browse: bool,
    /// Pooled runs: use the npx fast path (off measures sharing alone).
    pub fast_npx: bool,
    /// End sessions by killing the agent (a crash) instead of closing stdin.
    pub kill_agents: bool,
}

fn require_job_accounting() -> Result<()> {
    if !cfg!(windows) {
        bail!("hangar bench needs Windows job objects to count every process a run starts; it does not run on {}", std::env::consts::OS);
    }
    Ok(())
}

/// One measured run. Must be called in a process of its own (see module docs).
pub async fn run_once(cfg: RunConfig) -> Result<RunResult> {
    require_job_accounting()?;
    let job = Job::new_kill_on_close()?;
    job.assign_self().context("joining the measurement job")?;
    let me = std::process::id();
    let started = Instant::now();
    let cfg = Arc::new(cfg);

    let pool = Arc::new(match cfg.mode {
        Mode::Pooled => Some(start_pool(&cfg.workdir, cfg.fast_npx).await?),
        Mode::Direct => None,
    });

    // Every session starts at once: the "many sessions start together" case.
    let (sessions, session_ready_ms, mut failures) = launch_wave(&cfg, &pool, 0).await;
    tokio::time::sleep(cfg.settle).await;

    // Price everything in the job except this process.
    let snap = procs::snapshot();
    let in_job: HashSet<u32> = job.pids().into_iter().collect();
    let mine = snap.iter().find(|p| p.pid == me).map(|p| p.cpu_ms).unwrap_or(0);
    let cpu_ms = job.cpu_ms().saturating_sub(mine);
    let daemon_pid = pool.as_ref().as_ref().map(|p| p.daemon_pid);
    let agent_pids: HashSet<u32> = sessions.iter().filter_map(Session::agent_pid).collect();
    let mut breakdown: BTreeMap<String, Usage> = BTreeMap::new();
    let (mut mcp, mut agent_procs) = (Vec::new(), Vec::new());
    for p in snap.iter().filter(|p| in_job.contains(&p.pid) && p.pid != me) {
        let kind = if agent_pids.contains(&p.pid) {
            "agent"
        } else if Some(p.pid) == daemon_pid {
            "daemon"
        } else if p.name.to_ascii_lowercase().starts_with("hangar-shim") {
            "shim"
        } else {
            "server"
        };
        let entry = breakdown.entry(kind.to_string()).or_default();
        entry.processes += 1;
        entry.private_bytes += p.private_bytes;
        entry.working_set += p.working_set;
        entry.cpu_ms += p.cpu_ms;
        if kind == "agent" { agent_procs.push(p) } else { mcp.push(p) }
    }

    // Second wave: new sessions joining servers that are already warm.
    let (warm_sessions, warm_ready_ms) = if cfg.warm_wave && cfg.mode == Mode::Pooled {
        let (s, ms, errs) = launch_wave(&cfg, &pool, 1000).await;
        failures.extend(errs.into_iter().map(|e| format!("warm wave: {e}")));
        (s, ms)
    } else {
        (Vec::new(), Vec::new())
    };

    // End every session the way an agent does, then look for survivors.
    for s in sessions.into_iter().chain(warm_sessions) {
        s.close(cfg.kill_agents).await;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let kept = match pool.as_ref() {
        Some(p) => kept_warm(p).await,
        None => Vec::new(),
    };
    let snap = procs::snapshot();
    let expected: HashSet<u32> = procs::trees(&snap, &kept).iter().map(|p| p.pid).chain(daemon_pid).collect();
    let in_job: HashSet<u32> = job.pids().into_iter().collect();
    let survivors: Vec<&ProcInfo> = snap.iter().filter(|p| in_job.contains(&p.pid) && p.pid != me && !expected.contains(&p.pid)).collect();

    let result = RunResult {
        mode: cfg.mode,
        agent: cfg.agent,
        sessions: cfg.sessions,
        servers: cfg.servers.iter().map(|(n, _)| n.clone()).collect(),
        session_ready_ms,
        failures,
        mcp: Usage::of(mcp),
        agents: Usage::of(agent_procs),
        breakdown,
        cpu_ms,
        leftovers: Usage::of(survivors.iter().copied()),
        leftover_names: survivors.iter().map(|p| p.name.clone()).collect(),
        warm_ready_ms,
        wall_ms: started.elapsed().as_millis() as u64,
    };
    // Clean up everything else in the job. Terminating the job would also
    // end this process before it reports, and so would closing it: a
    // kill-on-close job we belong to stays open until we exit.
    for pid in job.pids() {
        if pid != me {
            procs::kill_tree(pid);
        }
    }
    std::mem::forget(job);
    Ok(result)
}

/// Starts `cfg.sessions` sessions together. Returns them, their ready times
/// and what went wrong.
async fn launch_wave(cfg: &Arc<RunConfig>, pool: &Arc<Option<Pool>>, first_index: usize) -> (Vec<Session>, Vec<Option<u64>>, Vec<String>) {
    let mut set = tokio::task::JoinSet::new();
    for i in 0..cfg.sessions {
        let (cfg, pool) = (cfg.clone(), pool.clone());
        set.spawn(async move { (i, agents::launch(first_index + i, &cfg, pool.as_ref().as_ref()).await) });
    }
    let mut sessions = Vec::new();
    let mut ready_ms = vec![None; cfg.sessions];
    let mut failures = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((i, Ok((session, ms)))) => {
                ready_ms[i] = ms;
                if ms.is_none() {
                    failures.extend(session.errors().iter().map(|e| format!("session {i}: {e}")));
                }
                sessions.push(session);
            }
            Ok((i, Err(e))) => failures.push(format!("session {i}: {e:#}")),
            Err(e) => failures.push(format!("session task: {e}")),
        }
    }
    (sessions, ready_ms, failures)
}

/// Upstreams the pool keeps running after their sessions leave (shared
/// servers wait for their idle timeout). Session-scoped ones must be gone.
async fn kept_warm(pool: &Pool) -> Vec<u32> {
    let Ok(status) = crate::daemon::send_control(&pool.home, "status", json!({})).await else { return Vec::new() };
    let servers = status["servers"].as_array().cloned().unwrap_or_default();
    servers
        .iter()
        .filter(|s| matches!(s["share"].as_str(), Some("global" | "project" | "native")))
        .filter_map(|s| s["pid"].as_u64().map(|p| p as u32))
        .collect()
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
    pub browse: bool,
    pub fast_npx: bool,
    pub kill_agents: bool,
}

/// A conservative guess at one server's memory before any run measured it
/// (Node-based servers with their npx launchers run 60–170 MB each).
const FIRST_RUN_PER_SERVER: u64 = 170 * 1024 * 1024;
/// A conservative guess at one headless Claude Code process.
const FIRST_RUN_PER_AGENT: u64 = 400 * 1024 * 1024;

/// Runs every (sessions, mode) combination, each in a fresh child process.
pub async fn run(opts: BenchOpts) -> Result<Vec<RunResult>> {
    require_job_accounting()?;
    let workdir = std::env::temp_dir().join("hangar-bench");
    std::fs::create_dir_all(&workdir)?;
    let servers = preset(&opts.preset, &workdir)?;
    println!(
        "hangar bench: {} {} servers ({}), agent={}, sessions={:?}",
        servers.len(),
        opts.preset,
        servers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
        opts.agent,
        opts.sessions
    );

    if opts.warmup {
        println!("warming the package cache (one direct start per server)...");
        warmup(&servers, &workdir).await?;
    }

    let exe = std::env::current_exe()?;
    let mut results: Vec<RunResult> = Vec::new();
    for &n in &opts.sessions {
        for &mode in &opts.modes {
            // A pooled run never needs more than the direct run that just fit.
            let direct_fit = results.iter().any(|r| r.sessions == n && r.mode == Mode::Direct);
            if !(mode == Mode::Pooled && direct_fit) {
                let need = per_session_estimate(&results, mode, opts.agent, servers.len()) * n as u64;
                let (_, avail) = procs::system_memory();
                if need + opts.reserve_bytes > avail {
                    println!(
                        "  skip {mode:>6} x{n:<3} needs ~{} plus a {} reserve; only {} is free",
                        fmt_bytes(need),
                        fmt_bytes(opts.reserve_bytes),
                        fmt_bytes(avail)
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
            for (on, flag) in [(opts.warm_wave, "--warm-wave"), (opts.browse, "--browse"), (!opts.fast_npx, "--no-fast-npx")] {
                if on {
                    cmd.arg(flag);
                }
            }
            if opts.kill_agents {
                cmd.args(["--teardown", "kill"]);
            }
            if let Some(c) = &opts.claude_bin {
                cmd.arg("--claude-bin").arg(c);
            }
            let status = cmd.status().await?;
            let text = std::fs::read_to_string(&out).with_context(|| format!("run {mode} x{n} produced no result (exit {status})"))?;
            let r: RunResult = serde_json::from_str(&text)?;
            println!("  {}", report::summary_line(&r));
            for f in r.failures.iter().take(5) {
                println!("      {f}");
            }
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

/// Memory one session of `mode` is expected to need: the largest measured so
/// far, or a conservative guess before the first run.
fn per_session_estimate(results: &[RunResult], mode: Mode, agent: Agent, servers: usize) -> u64 {
    let measured = results
        .iter()
        .filter(|r| r.mode == mode && r.agent == agent && r.sessions > 0)
        .map(|r| (r.mcp.private_bytes + r.agents.private_bytes) / r.sessions as u64)
        .max();
    measured.unwrap_or(FIRST_RUN_PER_SERVER * servers as u64 + if agent == Agent::Claude { FIRST_RUN_PER_AGENT } else { 0 })
}

/// Starts every server once so the first measured run does not pay for
/// package downloads. Everything it starts lives in a job that is
/// terminated afterwards, so nothing survives into the measured runs.
async fn warmup(servers: &ServerSet, workdir: &Path) -> Result<()> {
    let job = Job::new_kill_on_close()?;
    let record = agents::npx_record_dir(workdir);
    std::fs::create_dir_all(&record)?;
    let record = &record;
    let mut tasks = Vec::new();
    for (name, spec) in servers {
        let cmd = server_command(name, spec, workdir, None);
        let (name, root, job) = (name.clone(), workdir.display().to_string(), &job);
        tasks.push(async move {
            let Ok(client) = McpClient::spawn(cmd, vec![root]) else { return (name, false) };
            let _ = job.assign_pid(client.pid());
            let ok = client.initialize("hangar-warmup", READY_TIMEOUT).await.is_ok();
            client.shutdown_like_agent().await;
            if ok {
                // Real npx just resolved this package: as on any machine that
                // used it recently, pooled runs may start the cached copy.
                crate::launch::note_npx_ran(Some(record.as_path()), spec);
            }
            (name, ok)
        });
    }
    for (name, ok) in agents::join_all(tasks).await {
        if !ok {
            println!("  warning: {name} did not start during warmup");
        }
    }
    job.terminate();
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(mode: Mode, sessions: usize, mcp_mb: u64) -> RunResult {
        RunResult {
            mode,
            agent: Agent::Sim,
            sessions,
            servers: vec!["a".into()],
            session_ready_ms: vec![Some(100); sessions],
            failures: vec![],
            mcp: Usage { processes: sessions, private_bytes: mcp_mb << 20, working_set: 0, cpu_ms: 0 },
            agents: Usage::default(),
            breakdown: BTreeMap::new(),
            cpu_ms: 1000,
            leftovers: Usage::default(),
            leftover_names: vec![],
            warm_ready_ms: vec![],
            wall_ms: 0,
        }
    }

    #[test]
    fn estimates_start_conservative_then_use_measurements() {
        assert_eq!(per_session_estimate(&[], Mode::Direct, Agent::Sim, 5), 5 * FIRST_RUN_PER_SERVER);
        assert_eq!(per_session_estimate(&[], Mode::Direct, Agent::Claude, 1), FIRST_RUN_PER_SERVER + FIRST_RUN_PER_AGENT);
        let runs = [result(Mode::Direct, 2, 300), result(Mode::Direct, 4, 400)];
        assert_eq!(per_session_estimate(&runs, Mode::Direct, Agent::Sim, 5), 150 << 20, "the worst measured per-session cost");
    }

    #[test]
    fn results_round_trip_through_json() {
        let r = result(Mode::Pooled, 3, 90);
        let back: RunResult = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.sessions, 3);
        assert_eq!(back.mode, Mode::Pooled);
    }

    #[test]
    fn comparisons_skip_runs_with_failures() {
        let opts = BenchOpts {
            sessions: vec![2],
            preset: "light".into(),
            modes: vec![Mode::Direct, Mode::Pooled],
            agent: Agent::Sim,
            settle: Duration::ZERO,
            warm_wave: false,
            warmup: false,
            reserve_bytes: 0,
            out: None,
            claude_bin: None,
            browse: false,
            fast_npx: true,
            kill_agents: false,
        };
        let servers = vec![("a".to_string(), npx_spec(&["x"]))];
        let ok = render_markdown(&[result(Mode::Direct, 2, 300), result(Mode::Pooled, 2, 100)], &opts, &servers);
        assert!(ok.contains("**−67%**"), "{ok}");
        let mut broken = result(Mode::Pooled, 2, 100);
        broken.session_ready_ms[1] = None;
        let md = render_markdown(&[result(Mode::Direct, 2, 300), broken], &opts, &servers);
        assert!(md.contains("not compared"), "{md}");
        assert!(!md.contains("**−67%**"));
    }
}
