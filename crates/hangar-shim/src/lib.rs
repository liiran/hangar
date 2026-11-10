//! `hangar-shim`: what an MCP client launches instead of a stdio server.
//!
//! ```text
//! hangar-shim [--name NAME] [--session ID] [--env-keys A,B] -- <command> [args...]
//! ```
//!
//! The shim connects to the hangar daemon (starting it if needed), says which
//! server it stands in for, and then copies newline-delimited JSON-RPC between
//! its stdio and the daemon. It holds no MCP state of its own apart from the
//! client's `initialize` request, which it replays if the daemon restarts.
//!
//! If the daemon cannot be reached, the shim runs the real server itself with
//! inherited stdio, so wrapping a server never makes it stop working.

use hangar_proto::{read_daemon_info, DaemonInfo, Hello, ServerSpec, Welcome, ENV_BIN, ENV_NO_AUTOSTART, ENV_SESSION, WIRE_VERSION};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CONNECT_WAIT: Duration = Duration::from_secs(10);
const RECONNECT_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub name: String,
    pub session: Option<String>,
    pub env_keys: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
}

impl Opts {
    pub fn parse<I: IntoIterator<Item = String>>(argv: I) -> Result<Opts, String> {
        let mut opts = Opts::default();
        let mut it = argv.into_iter();
        let mut rest: Vec<String> = Vec::new();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--name" => opts.name = it.next().ok_or("--name needs a value")?,
                "--session" => opts.session = Some(it.next().ok_or("--session needs a value")?),
                "--env-keys" => {
                    let v = it.next().ok_or("--env-keys needs a value")?;
                    opts.env_keys = v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
                }
                "--" => {
                    rest.extend(it.by_ref());
                    break;
                }
                "-h" | "--help" => return Err(USAGE.into()),
                other if other.starts_with("--") && rest.is_empty() => return Err(format!("unknown option {other}\n{USAGE}")),
                _ => {
                    rest.push(arg);
                    rest.extend(it.by_ref());
                    break;
                }
            }
        }
        let mut rest = rest.into_iter();
        opts.command = rest.next().ok_or_else(|| format!("missing server command\n{USAGE}"))?;
        opts.args = rest.collect();
        if opts.name.is_empty() {
            opts.name = default_name(&opts.command, &opts.args);
        }
        if opts.session.is_none() {
            opts.session = std::env::var(ENV_SESSION).ok().filter(|s| !s.is_empty());
        }
        Ok(opts)
    }

    fn spec(&self) -> ServerSpec {
        let env = self
            .env_keys
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
            .collect();
        ServerSpec { command: self.command.clone(), args: self.args.clone(), env }
    }
}

const USAGE: &str = "usage: hangar-shim [--name NAME] [--session ID] [--env-keys A,B] -- <command> [args...]";

fn default_name(command: &str, args: &[String]) -> String {
    let launcher = |a: &str| matches!(a.to_ascii_lowercase().as_str(), "/c" | "/d" | "/s" | "npx" | "npx.cmd" | "uvx" | "pnpx" | "bunx" | "dlx" | "exec");
    let base = |s: &str| s.rsplit(['/', '\\']).next().unwrap_or(s).to_string();
    let Some(pkg) = args.iter().find(|a| !a.starts_with('-') && !launcher(a)) else {
        return base(command);
    };
    if pkg.contains('\\') || pkg.ends_with(".js") || pkg.ends_with(".py") || pkg.ends_with(".exe") {
        return base(pkg);
    }
    // npm package: strip the version, then prefer a meaningful segment.
    let bare = pkg.trim_start_matches('@');
    let bare = bare.split('@').next().unwrap_or(bare);
    let mut parts = bare.split('/');
    let first = parts.next().unwrap_or(bare);
    match parts.next() {
        Some(last) if matches!(last, "mcp" | "server" | "mcp-server" | "cli") => first.to_string(),
        Some(last) => last.trim_start_matches("server-").to_string(),
        None => first.to_string(),
    }
}

pub fn run() -> ExitCode {
    let opts = match Opts::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("hangar-shim: {e}");
            return ExitCode::from(2);
        }
    };
    let home = hangar_proto::home_dir();
    match connect(&opts, &home, None) {
        Ok((stream, reader, welcome)) if !welcome.direct => pump(stream, reader, &opts, &home),
        Ok(_) => run_direct(&opts),
        Err(e) => {
            eprintln!("hangar: pool unavailable ({e}); starting {} directly", opts.name);
            run_direct(&opts)
        }
    }
}

type Conn = (TcpStream, BufReader<TcpStream>, Welcome);

/// Connects to the daemon, starting it once if nothing answers.
fn connect(opts: &Opts, home: &Path, resume: Option<&serde_json::Value>) -> Result<Conn, String> {
    let deadline = Instant::now() + if resume.is_some() { RECONNECT_WAIT } else { CONNECT_WAIT };
    let mut started = false;
    loop {
        if let Some(info) = read_daemon_info(home) {
            let addr = SocketAddr::from(([127, 0, 0, 1], info.shim_port));
            if let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                return handshake(stream, &info, opts, resume);
            }
        }
        if !started {
            started = true;
            if let Err(e) = start_daemon(home) {
                // On first connect, fall back to running the server directly
                // right away. When reconnecting, someone else may bring a
                // daemon back, so keep waiting.
                if resume.is_none() {
                    return Err(e);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for the hangar daemon".into());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn handshake(stream: TcpStream, info: &DaemonInfo, opts: &Opts, resume: Option<&serde_json::Value>) -> Result<Conn, String> {
    let _ = stream.set_nodelay(true);
    let hello = Hello {
        hangar: WIRE_VERSION,
        token: info.token.clone(),
        name: opts.name.clone(),
        spec: opts.spec(),
        cwd: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        env: std::env::vars().collect::<BTreeMap<_, _>>(),
        session: opts.session.clone(),
        client_pid: parent_pid(),
        shim_pid: std::process::id(),
        resume: resume.cloned(),
    };
    let mut line = serde_json::to_vec(&hello).map_err(|e| e.to_string())?;
    line.push(b'\n');
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    writer.write_all(&line).map_err(|e| format!("handshake failed: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(64 * 1024, stream.try_clone().map_err(|e| e.to_string())?);
    let mut answer = String::new();
    reader.read_line(&mut answer).map_err(|e| format!("no answer from daemon: {e}"))?;
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    let welcome: Welcome = serde_json::from_str(answer.trim()).map_err(|e| format!("bad answer from daemon: {e}"))?;
    if !welcome.ok {
        return Err(welcome.error.unwrap_or_else(|| "daemon refused the connection".into()));
    }
    Ok((writer, reader, welcome))
}

/// Copies stdin to the daemon and the daemon to stdout until either side ends.
fn pump(stream: TcpStream, reader: BufReader<TcpStream>, opts: &Opts, home: &Path) -> ExitCode {
    let state = Arc::new(Pump { writer: Mutex::new((0, Some(stream))), stdin_closed: AtomicBool::new(false), init: Mutex::new(None) });

    let up = state.clone();
    std::thread::spawn(move || stdin_to_daemon(&up));

    let mut reader = reader;
    let stdout = std::io::stdout();
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf);
        match read {
            Ok(n) if n > 0 => {
                let mut out = stdout.lock();
                if out.write_all(&buf).and_then(|_| out.flush()).is_err() {
                    return ExitCode::SUCCESS; // the client went away
                }
            }
            _ => {
                if state.stdin_closed.load(Ordering::SeqCst) {
                    return ExitCode::SUCCESS;
                }
                // The daemon went away while the client is still running.
                // Reconnect and replay the client's initialize request so the
                // client never notices.
                // Park writers until a new connection exists, so requests sent
                // meanwhile wait instead of vanishing into the dead socket.
                {
                    let mut w = state.writer.lock().unwrap();
                    *w = (w.0 + 1, None);
                }
                let init = state.init.lock().unwrap().clone();
                let Some(init) = init else {
                    eprintln!("hangar: lost the daemon before initialize; exiting");
                    return ExitCode::from(1);
                };
                match connect(opts, home, Some(&init)) {
                    Ok((stream, new_reader, welcome)) if !welcome.direct => {
                        let mut w = state.writer.lock().unwrap();
                        *w = (w.0 + 1, Some(stream));
                        reader = new_reader;
                        eprintln!("hangar: reconnected to the daemon");
                    }
                    Ok(_) | Err(_) => {
                        eprintln!("hangar: lost the daemon and could not reconnect; exiting");
                        return ExitCode::from(1);
                    }
                }
            }
        }
    }
}

struct Pump {
    /// The current daemon socket, tagged with a generation that bumps on reconnect.
    writer: Mutex<(u64, Option<TcpStream>)>,
    stdin_closed: AtomicBool,
    init: Mutex<Option<serde_json::Value>>,
}

fn stdin_to_daemon(state: &Pump) {
    let stdin = std::io::stdin();
    let mut input = BufReader::with_capacity(64 * 1024, stdin.lock());
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut captured = false;
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !captured {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) {
                if v.get("method").and_then(|m| m.as_str()) == Some("initialize") && v.get("id").is_some() {
                    *state.init.lock().unwrap() = Some(v);
                    captured = true;
                }
            }
        }
        if !buf.ends_with(b"\n") {
            buf.push(b'\n');
        }
        write_to_daemon(state, &buf);
    }
    state.stdin_closed.store(true, Ordering::SeqCst);
    if let Some(w) = state.writer.lock().unwrap().1.as_ref() {
        let _ = w.shutdown(Shutdown::Write);
    }
    // Never outlive the client by more than a moment.
    std::thread::sleep(Duration::from_secs(3));
    std::process::exit(0);
}

/// Writes one line; on failure waits for the reader thread to reconnect and retries.
fn write_to_daemon(state: &Pump, line: &[u8]) {
    let deadline = Instant::now() + RECONNECT_WAIT;
    let mut failed_gen: Option<u64> = None;
    loop {
        {
            let guard = state.writer.lock().unwrap();
            let (generation, writer) = (&guard.0, &guard.1);
            if let Some(mut w) = writer.as_ref() {
                if failed_gen != Some(*generation) {
                    if w.write_all(line).is_ok() {
                        return;
                    }
                    failed_gen = Some(*generation);
                }
            }
        }
        if Instant::now() >= deadline {
            return; // dropped; the client will time out this request
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts `hangar daemon` detached from the client's process tree.
fn start_daemon(home: &Path) -> Result<(), String> {
    if std::env::var(ENV_NO_AUTOSTART).map(|v| v == "1").unwrap_or(false) {
        return Err("no daemon running and autostart is disabled".into());
    }
    let bin = hangar_bin().ok_or("could not find the hangar binary")?;
    let mut cmd = Command::new(&bin);
    cmd.arg("daemon").arg("--quiet").env(hangar_proto::ENV_HOME, home).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
        // The client's job may forbid breakaway; start it inside the job instead.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("could not start {}: {e}", bin.display()))
}

fn hangar_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(ENV_BIN).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let exe = if cfg!(windows) { "hangar.exe" } else { "hangar" };
    if let Ok(me) = std::env::current_exe() {
        let sibling = me.with_file_name(exe);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    resolve_command("hangar").filter(|p| p.is_file())
}

/// Runs the real server with inherited stdio. Used when pooling is unavailable
/// or disabled for this server.
fn run_direct(opts: &Opts) -> ExitCode {
    #[cfg(windows)]
    contain_children();
    let program = resolve_command(&opts.command).unwrap_or_else(|| PathBuf::from(&opts.command));
    let mut cmd = Command::new(&program);
    cmd.args(&opts.args).stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        // The shim has no console, so a console child would get a new,
        // visible one. Give it a hidden console instead.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("hangar-shim: could not start {}: {e}", program.display());
            ExitCode::from(127)
        }
    }
}

/// Finds a bare command on PATH the way a shell would, including `.cmd`
/// wrappers such as `npx.cmd` on Windows.
pub fn resolve_command(command: &str) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.components().count() > 1 || path.is_absolute() {
        return Some(path.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        if path.extension().is_some() {
            vec![String::new()]
        } else {
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').map(|e| e.to_ascii_lowercase()).collect()
        }
    } else {
        vec![String::new()]
    };
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        for ext in &exts {
            let candidate = dir.join(format!("{command}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn parent_pid() -> Option<u32> {
    #[cfg(windows)]
    {
        win::parent_pid(std::process::id())
    }
    #[cfg(unix)]
    {
        Some(std::os::unix::process::parent_id())
    }
}

/// Puts the shim in a kill-on-close job so a directly started server dies with it.
#[cfg(windows)]
fn contain_children() {
    win::kill_children_on_exit();
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn parent_pid(pid: u32) -> Option<u32> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = None;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    if entry.th32ProcessID == pid {
                        found = Some(entry.th32ParentProcessID);
                        break;
                    }
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
            found
        }
    }

    pub fn kill_children_on_exit() {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                CloseHandle(job);
            }
            // The handle is intentionally leaked: it closes when the shim exits,
            // which kills the server we started.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Opts, String> {
        Opts::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_options_and_command() {
        let o = parse(&["--name", "pw", "--env-keys", "A, B", "--", "cmd", "/c", "npx", "-y", "@playwright/mcp@latest"]).unwrap();
        assert_eq!(o.name, "pw");
        assert_eq!(o.env_keys, vec!["A", "B"]);
        assert_eq!(o.command, "cmd");
        assert_eq!(o.args, vec!["/c", "npx", "-y", "@playwright/mcp@latest"]);
    }

    #[test]
    fn command_without_separator() {
        let o = parse(&["node", "server.js", "--flag"]).unwrap();
        assert_eq!(o.command, "node");
        assert_eq!(o.args, vec!["server.js", "--flag"]);
    }

    #[test]
    fn args_after_command_are_not_options() {
        let o = parse(&["--", "npx", "--name", "x"]).unwrap();
        assert_eq!(o.args, vec!["--name", "x"]);
    }

    #[test]
    fn missing_command_is_an_error() {
        assert!(parse(&["--name", "x"]).is_err());
        assert!(parse(&["--bogus", "x"]).is_err());
    }

    #[test]
    fn default_names_come_from_the_package() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(default_name("cmd", &a(&["/c", "npx", "-y", "@playwright/mcp@latest"])), "playwright");
        assert_eq!(default_name("npx", &a(&["-y", "chrome-devtools-mcp@latest"])), "chrome-devtools-mcp");
        assert_eq!(default_name("npx", &a(&["-y", "@modelcontextprotocol/server-memory"])), "memory");
        assert_eq!(default_name("C:/tools/server.exe", &[]), "server.exe");
        assert_eq!(default_name("node", &a(&[r"C:\srv\index.js"])), "index.js");
    }
}
