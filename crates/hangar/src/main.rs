use anyhow::Result;
use clap::{Parser, Subcommand};
use hangar::bench::{self, fmt_bytes, Agent, Mode};
use hangar::config::Config;
use hangar::daemon::{self, AlreadyRunning, Daemon, DaemonOptions};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "hangar", version, about = "Run many coding-agent sessions on one machine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground. Shims start it on demand.
    Daemon {
        /// Log to ~/.hangar/logs/daemon.log instead of the terminal.
        #[arg(long)]
        quiet: bool,
    },
    /// Show the daemon and every pooled server.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Stop the daemon and every server it runs.
    Stop,
    /// Measure MCP memory, CPU and startup time for N sessions, direct vs pooled.
    Bench {
        /// Session counts to measure.
        #[arg(long, value_delimiter = ',', default_value = "1,5,10")]
        sessions: Vec<usize>,
        /// `popular`, `light`, or a path to a JSON file with `mcpServers`.
        #[arg(long, default_value = "popular")]
        preset: String,
        #[arg(long, value_enum, value_delimiter = ',', default_value = "direct,pooled")]
        modes: Vec<Mode>,
        /// `sim` (an MCP client per server) or `claude` (real headless Claude Code; no tokens used).
        #[arg(long, value_enum, default_value = "sim")]
        agent: Agent,
        /// Wait this long after the last session is ready before measuring.
        #[arg(long, default_value_t = 3000)]
        settle_ms: u64,
        /// Pooled runs: also time a second wave of sessions joining warm servers.
        #[arg(long)]
        warm_wave: bool,
        #[arg(long)]
        no_warmup: bool,
        /// Skip a run if it would leave less than this much memory free.
        #[arg(long, default_value_t = 2.0)]
        reserve_gb: f64,
        /// Write results to this path (.json and .md).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        claude_bin: Option<PathBuf>,
        /// Sim agent: every session opens a page in its browser (and checks isolation).
        #[arg(long)]
        browse: bool,
        /// Pooled runs: turn off the npx fast path, to measure sharing alone.
        #[arg(long)]
        no_fast_npx: bool,
        /// How sessions end: `close` (stdin) or `kill` (crash; Claude agent).
        #[arg(long, default_value = "close")]
        teardown: String,
    },
    /// One measured run (used internally by `bench`).
    #[command(hide = true)]
    BenchRun {
        #[arg(long, value_enum)]
        mode: Mode,
        #[arg(long, value_enum)]
        agent: Agent,
        #[arg(long)]
        sessions: usize,
        #[arg(long)]
        preset: String,
        #[arg(long)]
        workdir: PathBuf,
        #[arg(long)]
        json: PathBuf,
        #[arg(long, default_value_t = 3000)]
        settle_ms: u64,
        #[arg(long)]
        warm_wave: bool,
        #[arg(long)]
        claude_bin: Option<PathBuf>,
        #[arg(long)]
        browse: bool,
        #[arg(long)]
        no_fast_npx: bool,
        #[arg(long, default_value = "close")]
        teardown: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = hangar::proto::home_dir();
    match cli.command {
        Command::Daemon { quiet } => run_daemon(home, quiet).await?,
        Command::Status { json } => status(&home, json).await?,
        Command::Stop => {
            daemon::send_control(&home, "stop", json!({})).await?;
            println!("hangar daemon stopping");
        }
        Command::Bench { sessions, preset, modes, agent, settle_ms, warm_wave, no_warmup, reserve_gb, out, claude_bin, browse, no_fast_npx, teardown } => {
            bench::run(bench::BenchOpts {
                sessions,
                preset,
                modes,
                agent,
                settle: Duration::from_millis(settle_ms),
                warm_wave,
                warmup: !no_warmup,
                reserve_bytes: (reserve_gb * 1024.0 * 1024.0 * 1024.0) as u64,
                out,
                claude_bin,
                browse,
                fast_npx: !no_fast_npx,
                kill_agents: teardown == "kill",
            })
            .await?;
        }
        Command::BenchRun { mode, agent, sessions, preset, workdir, json, settle_ms, warm_wave, claude_bin, browse, no_fast_npx, teardown } => {
            let servers = bench::preset(&preset, &workdir)?;
            let result = bench::run_once(bench::RunConfig {
                mode,
                agent,
                sessions,
                servers,
                workdir,
                settle: Duration::from_millis(settle_ms),
                warm_wave,
                claude_bin,
                browse,
                fast_npx: !no_fast_npx,
                kill_agents: teardown == "kill",
            })
            .await?;
            std::fs::write(&json, serde_json::to_vec_pretty(&result)?)?;
        }
    }
    Ok(())
}

fn init_logging(home: &std::path::Path, quiet: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("HANGAR_LOG").unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    if quiet {
        let dir = home.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("daemon.log")) {
            builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
            return;
        }
    }
    builder.with_writer(std::io::stderr).init();
}

async fn run_daemon(home: std::path::PathBuf, quiet: bool) -> Result<()> {
    init_logging(&home, quiet);
    // Everything this process starts lives in a kill-on-close job, so no
    // server can outlive the daemon, even if the daemon crashes.
    if let Ok(job) = hangar::procs::Job::new_kill_on_close() {
        if job.assign_self().is_ok() {
            std::mem::forget(job);
        }
    }
    let config = Config::load(&home)?;
    let daemon = match Daemon::start(DaemonOptions { home, config }).await {
        Ok(d) => d,
        Err(e) if e.is::<AlreadyRunning>() => {
            if !quiet {
                eprintln!("{e}");
            }
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    tokio::select! {
        _ = daemon.stopped() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    daemon.shutdown().await;
    Ok(())
}

async fn status(home: &std::path::Path, as_json: bool) -> Result<()> {
    let v = match daemon::send_control(home, "status", json!({})).await {
        Ok(v) => v,
        Err(e) => {
            println!("{e:#}");
            return Ok(());
        }
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let d = &v["daemon"];
    let up = hangar::pool::upstream::now_ms().saturating_sub(d["started_at_ms"].as_u64().unwrap_or(0)) / 1000;
    println!(
        "hangar daemon {} (pid {}), up {}, {}",
        d["version"].as_str().unwrap_or("?"),
        d["pid"],
        fmt_duration(up),
        fmt_bytes(v["daemon_memory"]["private_bytes"].as_u64().unwrap_or(0))
    );
    let servers = v["servers"].as_array().cloned().unwrap_or_default();
    if servers.is_empty() {
        println!("no servers running");
        return Ok(());
    }
    println!("
{:<22} {:<8} {:<10} {:>7} {:>8} {:>10} {:>8} {:>6} {:>8}", "SERVER", "SHARE", "STATE", "PID", "SESSIONS", "MEMORY", "REQUESTS", "CACHED", "RESTARTS");
    let mut total = 0u64;
    let mut sessions = 0usize;
    for s in &servers {
        let mem = s["memory"]["private_bytes"].as_u64().unwrap_or(0);
        total += mem;
        let n = s["sessions"].as_array().map(Vec::len).unwrap_or(0);
        sessions += n;
        println!(
            "{:<22} {:<8} {:<10} {:>7} {:>8} {:>10} {:>8} {:>6} {:>8}",
            truncate(s["name"].as_str().unwrap_or("?"), 22),
            s["share"].as_str().unwrap_or("?"),
            s["state"].as_str().unwrap_or("?"),
            s["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            n,
            fmt_bytes(mem),
            s["requests"],
            s["cache_hits"],
            s["restarts"],
        );
        if let Some(err) = s["last_error"].as_str() {
            println!("    last error: {err}");
        }
    }
    println!("
{} servers, {} session attachments, {} total", servers.len(), sessions, fmt_bytes(total));
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

fn fmt_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}
