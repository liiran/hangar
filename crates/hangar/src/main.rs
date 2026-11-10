use anyhow::Result;
use clap::{Parser, Subcommand};
use hangar::bench::{self, Agent, Mode};
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
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Bench { sessions, preset, modes, agent, settle_ms, warm_wave, no_warmup, reserve_gb, out, claude_bin } => {
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
            })
            .await?;
        }
        Command::BenchRun { mode, agent, sessions, preset, workdir, json, settle_ms, warm_wave, claude_bin } => {
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
            })
            .await?;
            std::fs::write(&json, serde_json::to_vec_pretty(&result)?)?;
        }
    }
    Ok(())
}
