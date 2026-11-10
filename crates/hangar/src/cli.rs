//! What the `hangar` commands do, apart from the benchmark and verifier.

use anyhow::{Context, Result, bail};
use hangar::config::Config;
use hangar::daemon::{self, AlreadyRunning, Daemon, DaemonOptions};
use hangar::util::fmt_bytes;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub async fn run_daemon(home: PathBuf, quiet: bool, ui_port: Option<u16>) -> Result<()> {
    init_logging(&home, quiet);
    // Everything this process starts lives in a kill-on-close job, so no
    // server can outlive the daemon, even if the daemon crashes.
    if let Ok(job) = hangar::procs::Job::new_kill_on_close()
        && job.assign_self().is_ok()
    {
        std::mem::forget(job);
    }
    let config = Config::load(&home)?;
    let daemon = match Daemon::start(DaemonOptions { home, config, ui_port }).await {
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

fn init_logging(home: &Path, quiet: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("HANGAR_LOG").unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    if quiet {
        let dir = home.join("logs");
        if std::fs::create_dir_all(&dir).is_ok() {
            hangar::util::make_private(&dir);
            if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("daemon.log")) {
                builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
                return;
            }
        }
    }
    builder.with_writer(std::io::stderr).init();
}

pub async fn stop(home: &Path) -> Result<()> {
    daemon::send_control(home, "stop", json!({})).await?;
    println!("hangar daemon stopping");
    Ok(())
}

pub async fn status(home: &Path, as_json: bool) -> Result<()> {
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
    let up = hangar::util::now_ms().saturating_sub(d["started_at_ms"].as_u64().unwrap_or(0)) / 1000;
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
    let row = |cols: [&str; 9]| {
        format!("{:<22} {:<8} {:<10} {:>7} {:>8} {:>10} {:>8} {:>6} {:>8}", cols[0], cols[1], cols[2], cols[3], cols[4], cols[5], cols[6], cols[7], cols[8])
    };
    println!("\n{}", row(["SERVER", "SHARE", "STATE", "PID", "SESSIONS", "MEMORY", "REQUESTS", "CACHED", "RESTARTS"]));
    let (mut total, mut sessions) = (0u64, 0usize);
    for s in &servers {
        let mem = s["memory"]["private_bytes"].as_u64().unwrap_or(0);
        let n = s["sessions"].as_array().map(Vec::len).unwrap_or(0);
        total += mem;
        sessions += n;
        println!(
            "{}",
            row([
                &truncate(s["name"].as_str().unwrap_or("?"), 22),
                s["share"].as_str().unwrap_or("?"),
                s["state"].as_str().unwrap_or("?"),
                &s["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                &n.to_string(),
                &fmt_bytes(mem),
                &s["requests"].to_string(),
                &s["cache_hits"].to_string(),
                &s["restarts"].to_string(),
            ])
        );
        if let Some(err) = s["last_error"].as_str() {
            println!("    last error: {err}");
        }
    }
    println!("\n{} servers, {} session attachments, {} total", servers.len(), sessions, fmt_bytes(total));
    Ok(())
}

/// Starts the daemon if it is not running and opens the app window.
pub async fn up(home: &Path, no_open: bool) -> Result<()> {
    let running = hangar::proto::read_daemon_info(home).is_some() && daemon::send_control(home, "status", json!({})).await.is_ok();
    if !running {
        spawn_detached_daemon(home)?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let info = loop {
        if let Some(i) = hangar::proto::read_daemon_info(home)
            && i.ui_port.is_some()
            && daemon::send_control(home, "status", json!({})).await.is_ok()
        {
            break i;
        }
        if Instant::now() > deadline {
            bail!("hangar did not start; see {}", home.join("logs").join("daemon.log").display());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let port = info.ui_port.unwrap_or(daemon::DEFAULT_UI_PORT);
    // A one-time code, not the token: the URL may end up in process lists
    // and browser history.
    let paired = daemon::send_control(home, "pair", json!({})).await?;
    let code = paired["code"].as_str().context("the daemon did not issue a pairing code")?;
    let url = format!("http://127.0.0.1:{port}/#pair={code}");
    if no_open {
        println!("{url}");
        return Ok(());
    }
    println!("hangar is running at http://127.0.0.1:{port}");
    open_app(&url);
    Ok(())
}

fn spawn_detached_daemon(home: &Path) -> Result<()> {
    // Our own stdio handles must not leak into the daemon, or the terminal
    // that ran `hangar up` would wait for it.
    hangar_shim::keep_std_handles_private();
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args(["daemon", "--quiet"]).env(hangar::proto::ENV_HOME, home);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x08;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_err() {
            // The job we are in does not allow breakaway.
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            cmd.spawn()?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0).spawn()?;
    }
    Ok(())
}

/// Opens `url` as a chromeless app window when Edge or Chrome is around,
/// otherwise in the default browser.
fn open_app(url: &str) {
    #[cfg(windows)]
    {
        let candidates = [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        ];
        for c in candidates {
            if Path::new(c).is_file() && std::process::Command::new(c).arg(format!("--app={url}")).spawn().is_ok() {
                return;
            }
        }
        let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

pub fn wrap(home: &Path, dry_run: bool, wrap: bool) -> Result<()> {
    use hangar::claude_config as cc;
    let path = cc::claude_json_path();
    let changes = if wrap {
        let shim = cc::shim_path()?;
        cc::edit_file(&path, home, dry_run, |doc, originals| cc::wrap_doc(doc, &shim, originals))?
    } else {
        cc::edit_file(&path, home, dry_run, cc::unwrap_doc)?
    };
    if changes.is_empty() {
        println!("{}", if wrap { "no stdio MCP servers to wrap" } else { "nothing is wrapped" });
        return Ok(());
    }
    for c in &changes {
        println!("{} {} ({})\n  {}\n  -> {}", if dry_run { "would change" } else { "changed" }, c.name, c.scope, c.before, c.after);
    }
    if !dry_run {
        println!("\n{} server(s) updated in {}. Restart Claude Code sessions to pick it up.", changes.len(), path.display());
        println!("A backup is in {}.", home.join("backups").display());
    }
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
