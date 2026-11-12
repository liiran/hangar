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
                &truncate(&hangar::util::printable(s["name"].as_str().unwrap_or("?")), 22),
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
            println!("    last error: {}", hangar::util::printable(err));
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
    open_app(home, &url);
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

/// Opens `url` in the default browser. In Chrome, Edge or another Chromium
/// browser that is hangar's own window: the installed app if the user
/// installed hangar from the browser (its own taskbar icon, a title bar in
/// the app's color), else a plain app window. Other browsers get a tab.
#[cfg_attr(windows, allow(unused_variables))]
fn open_app(home: &Path, url: &str) {
    #[cfg(windows)]
    {
        let origin = url.split("/#").next().unwrap_or(url);
        let app = |exe: &Path| {
            if !is_chromium(exe) {
                return false;
            }
            let mut cmd = std::process::Command::new(exe);
            match installed_app(exe, origin) {
                // The installed app, opened at the one-time link.
                Some((profile, id)) => cmd
                    .arg(format!("--profile-directory={profile}"))
                    .arg(format!("--app-id={id}"))
                    .arg(format!("--app-launch-url-for-shortcuts-menu-item={url}")),
                None => cmd.arg(format!("--app={url}")),
            };
            cmd.spawn().is_ok()
        };
        match default_browser() {
            Some(exe) if app(&exe) => return,
            Some(_) => {}
            // Not known: an app window in Chrome or Edge if either is there.
            None => {
                let candidates = [
                    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
                    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
                ];
                if candidates.iter().map(Path::new).any(|c| c.is_file() && app(c)) {
                    return;
                }
            }
        }
        let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
    }
    #[cfg(unix)]
    {
        // Other users can read a process's arguments here, so the browser
        // is given a private file that leads to the one-time link.
        let target = launcher(home, url).map_or_else(|| url.to_string(), |p| p.display().to_string());
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener).arg(target).spawn();
    }
}

/// A page only this user can read that sends the browser on to `url`. The
/// code in it is single-use and expires in two minutes.
#[cfg(any(unix, test))]
fn launcher(home: &Path, url: &str) -> Option<PathBuf> {
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>hangar</title><script>location.replace({})</script>",
        serde_json::to_string(url).ok()?.replace('<', concat!("\\", "u003c"))
    );
    let path = home.join("open.html");
    hangar::util::write_private(&path, page.as_bytes()).ok()?;
    Some(path)
}

/// The id Chromium browsers give an installed web app: its manifest id (for
/// hangar, its address with a trailing slash) hashed twice with SHA-256,
/// the first 16 bytes, each hex digit written as a letter from `a` to `p`.
#[cfg(any(windows, test))]
fn web_app_id(manifest_id: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(Sha256::digest(manifest_id.as_bytes())).iter().take(16).flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect()
}

/// The profile of `browser` in which hangar at `origin` is installed as an
/// app, with the app's id. The browser keeps a folder per installed app.
#[cfg(windows)]
fn installed_app(browser: &Path, origin: &str) -> Option<(String, String)> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let data = match browser.file_stem()?.to_string_lossy().to_ascii_lowercase().as_str() {
        "chrome" => local.join("Google").join("Chrome").join("User Data"),
        "msedge" => local.join("Microsoft").join("Edge").join("User Data"),
        "brave" => local.join("BraveSoftware").join("Brave-Browser").join("User Data"),
        _ => return None,
    };
    let id = web_app_id(&format!("{origin}/"));
    std::fs::read_dir(data)
        .ok()?
        .flatten()
        .find(|profile| profile.path().join("Web Applications").join(format!("_crx_{id}")).is_dir())
        .map(|profile| (profile.file_name().to_string_lossy().into_owned(), id))
}

/// The program that opens web links, as chosen in Windows settings.
#[cfg(windows)]
fn default_browser() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    let choice = r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice";
    let prog_id = registry_string(HKEY_CURRENT_USER, choice, Some("ProgId"))?;
    let command = registry_string(HKEY_CLASSES_ROOT, &format!(r"{prog_id}\shell\open\command"), None)?;
    let exe = command_program(&command);
    exe.is_file().then_some(exe)
}

/// A text value from the registry (`None` for a key's default value).
#[cfg(windows)]
fn registry_string(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: Option<&str>) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, value) = (wide(key), value.map(wide));
    let mut buf = vec![0u16; 2048];
    let mut bytes = (buf.len() * 2) as u32;
    let value_ptr = value.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    // SAFETY: the strings are NUL-terminated and `bytes` is the buffer's size.
    let rc = unsafe { RegGetValueW(root, key.as_ptr(), value_ptr, RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
    if rc != 0 {
        return None;
    }
    buf.truncate(bytes as usize / 2);
    Some(String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
}

/// The program a registry open command runs, as in
/// `"C:\…\chrome.exe" --single-argument %1`.
#[cfg(any(windows, test))]
fn command_program(command: &str) -> PathBuf {
    let c = command.trim();
    let exe = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next(),
        None => c.split(' ').next(),
    };
    exe.unwrap_or_default().into()
}

/// Browsers that can open a page as an app window (`--app=`).
#[cfg(any(windows, test))]
fn is_chromium(exe: &Path) -> bool {
    let name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["chrome.exe", "msedge.exe", "brave.exe", "vivaldi.exe", "chromium.exe"].contains(&name.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_page_leads_to_the_link() {
        let home = std::env::temp_dir().join(format!("hangar-launcher-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let path = launcher(&home, "http://127.0.0.1:7317/#pair=abc\"</script>").unwrap();
        let page = std::fs::read_to_string(&path).unwrap();
        assert!(page.contains(concat!(r#"location.replace("http://127.0.0.1:7317/#pair=abc\""#, "\\", r#"u003c/script>")"#)), "{page}");
        assert_eq!(page.matches("</script>").count(), 1, "the link cannot end the script early");
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn web_app_ids_match_the_ones_chrome_gives() {
        // Read from Chrome 157's profile folder after installing hangar from these addresses.
        for (address, id) in [
            ("http://127.0.0.1:51381/", "kdgcpaiokhdocediedmcodeobfhaocem"),
            ("http://127.0.0.1:53777/", "ahlljijehjmohjhjdincpfkfjlolhdnf"),
            ("http://127.0.0.1:53926/", "lalklbbfcihdnefdhkjiaebiicfhfahf"),
        ] {
            assert_eq!(web_app_id(address), id);
        }
    }

    #[test]
    fn the_default_browser_command_names_its_program() {
        let chrome = command_program(r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --single-argument %1"#);
        assert_eq!(chrome, PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"));
        assert!(is_chromium(&chrome));
        assert_eq!(command_program(r"C:\Firefox\firefox.exe -osint -url %1"), PathBuf::from(r"C:\Firefox\firefox.exe"));
        assert!(!is_chromium(Path::new(r"C:\Firefox\firefox.exe")), "other browsers get a tab");
        assert!(is_chromium(Path::new("MSEDGE.EXE")));
    }
}
