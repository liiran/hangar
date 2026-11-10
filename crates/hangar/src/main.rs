mod cli;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use hangar::bench::{self, Agent, Mode};
use hangar::daemon;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "hangar", version, about = "Run many coding-agent sessions on one machine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// How benchmark sessions end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Teardown {
    /// Close the agent's stdin, the normal way a session ends.
    Close,
    /// Kill the agent, like a crash.
    Kill,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground. Shims start it on demand.
    Daemon {
        /// Log to ~/.hangar/logs/daemon.log instead of the terminal.
        #[arg(long)]
        quiet: bool,
        /// Do not serve the web UI.
        #[arg(long)]
        no_ui: bool,
        /// Port for the web UI (falls back to any free port if taken).
        #[arg(long, default_value_t = daemon::DEFAULT_UI_PORT)]
        ui_port: u16,
    },
    /// Start hangar (if needed) and open the app.
    Up {
        /// Print the URL instead of opening a window.
        #[arg(long)]
        no_open: bool,
    },
    /// Route Claude Code's stdio MCP servers through the pool (~/.claude.json).
    Wrap {
        /// Show what would change without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Undo `hangar wrap`.
    Unwrap {
        /// Show what would change without writing.
        #[arg(long)]
        dry_run: bool,
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
        #[arg(long, value_enum, default_value = "close")]
        teardown: Teardown,
    },
    /// Check that pooled servers answer exactly like direct ones, and time calls.
    Verify {
        /// Latency calls per server and mode.
        #[arg(long, default_value_t = 200)]
        calls: usize,
        /// Also have real Claude Code (Haiku) do MCP tasks both ways. Costs a few cents.
        #[arg(long)]
        with_claude: bool,
        /// Runs per agent task and mode.
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        /// Write the report to this path (.json and .md).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        claude_bin: Option<PathBuf>,
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
        #[arg(long, value_enum, default_value = "close")]
        teardown: Teardown,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let home = hangar::proto::home_dir();
    match Cli::parse().command {
        Command::Daemon { quiet, no_ui, ui_port } => cli::run_daemon(home, quiet, (!no_ui).then_some(ui_port)).await?,
        Command::Up { no_open } => cli::up(&home, no_open).await?,
        Command::Wrap { dry_run } => cli::wrap(&home, dry_run, true)?,
        Command::Unwrap { dry_run } => cli::wrap(&home, dry_run, false)?,
        Command::Status { json } => cli::status(&home, json).await?,
        Command::Stop => cli::stop(&home).await?,
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
                kill_agents: teardown == Teardown::Kill,
            })
            .await?;
        }
        Command::Verify { calls, with_claude, repeat, out, claude_bin } => {
            let report = hangar::verify::run(hangar::verify::VerifyOpts { calls, with_claude, repeat, out, claude_bin }).await?;
            if !report.all_matched() {
                std::process::exit(1);
            }
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
                kill_agents: teardown == Teardown::Kill,
            })
            .await?;
            std::fs::write(&json, serde_json::to_vec_pretty(&result)?)?;
        }
    }
    Ok(())
}
