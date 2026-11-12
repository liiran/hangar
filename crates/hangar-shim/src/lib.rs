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

use hangar_proto::{Challenge, DaemonInfo, ENV_BIN, ENV_NO_AUTOSTART, ENV_SESSION, Hello, Proof, ServerSpec, WIRE_VERSION, Welcome, read_daemon_info};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
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
        let env = self.env_keys.iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v))).collect();
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
        Some("mcp" | "server" | "mcp-server" | "cli") => first.to_string(),
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
fn connect(opts: &Opts, home: &Path, resume: Option<&Value>) -> Result<Conn, String> {
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

fn write_json(w: &mut TcpStream, v: &impl serde::Serialize) -> Result<(), String> {
    let mut line = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    line.push(b'\n');
    w.write_all(&line).map_err(|e| format!("could not talk to the daemon: {e}"))
}

fn read_json<T: serde::de::DeserializeOwned>(r: &mut BufReader<TcpStream>) -> Result<T, String> {
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| format!("no answer from the daemon: {e}"))?;
    serde_json::from_str(line.trim()).map_err(|e| format!("bad answer from the daemon: {e}"))
}

/// Makes the daemon prove it knows the token before revealing anything to
/// it (a stale `daemon.json` could point at someone else's port), then
/// introduces the server this shim stands in for.
fn handshake(stream: TcpStream, info: &DaemonInfo, opts: &Opts, resume: Option<&Value>) -> Result<Conn, String> {
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(64 * 1024, stream.try_clone().map_err(|e| e.to_string())?);

    let nonce = hangar_proto::nonce();
    write_json(&mut writer, &Challenge { hangar: WIRE_VERSION, challenge: nonce.clone() })?;
    let answer: Proof = read_json(&mut reader)?;
    if !hangar_proto::constant_time_eq(&answer.proof, &hangar_proto::proof(&info.token, &nonce)) {
        return Err("the process on the hangar port is not the hangar daemon".into());
    }

    let hello = Hello {
        hangar: WIRE_VERSION,
        token: info.token.clone(),
        name: opts.name.clone(),
        spec: opts.spec(),
        cwd: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        // Variables that are not valid Unicode cannot be sent as JSON; they are left out.
        env: std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect::<BTreeMap<_, _>>(),
        session: opts.session.clone(),
        client_pid: parent_pid(),
        shim_pid: std::process::id(),
        resume: resume.cloned(),
    };
    write_json(&mut writer, &hello)?;
    let welcome: Welcome = read_json(&mut reader)?;
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    if !welcome.ok {
        return Err(welcome.error.unwrap_or_else(|| "daemon refused the connection".into()));
    }
    Ok((writer, reader, welcome))
}

/// The two fields of a JSON-RPC message the shim cares about.
#[derive(serde::Deserialize)]
struct Head {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
}

fn head(line: &[u8]) -> Option<Head> {
    serde_json::from_slice(line).ok()
}

struct Pump {
    /// The current daemon socket, tagged with a generation that bumps on reconnect.
    writer: Mutex<(u64, Option<TcpStream>)>,
    stdin_closed: AtomicBool,
    /// The client's initialize request and whether it has been answered.
    init: Mutex<Option<Value>>,
    init_answered: AtomicBool,
    /// Requests the client sent that have no answer yet: id key -> id.
    inflight: Mutex<HashMap<String, Value>>,
}

fn write_stdout(line: &[u8]) -> bool {
    let mut out = std::io::stdout().lock();
    out.write_all(line).and_then(|_| out.flush()).is_ok()
}

/// Copies stdin to the daemon and the daemon to stdout until either side ends.
fn pump(stream: TcpStream, reader: BufReader<TcpStream>, opts: &Opts, home: &Path) -> ExitCode {
    let state = Arc::new(Pump {
        writer: Mutex::new((0, Some(stream))),
        stdin_closed: AtomicBool::new(false),
        init: Mutex::new(None),
        init_answered: AtomicBool::new(false),
        inflight: Mutex::new(HashMap::new()),
    });

    let up = state.clone();
    std::thread::spawn(move || stdin_to_daemon(&up));

    let mut reader = reader;
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(n) if n > 0 => {
                // A response: no longer in flight.
                if let Some(h) = head(&buf)
                    && h.method.is_none()
                    && let Some(id) = h.id
                {
                    let key = id.to_string();
                    state.inflight.lock().unwrap().remove(&key);
                    let init_key = state.init.lock().unwrap().as_ref().and_then(|i| i.get("id")).map(Value::to_string);
                    if init_key.as_deref() == Some(key.as_str()) {
                        state.init_answered.store(true, Ordering::SeqCst);
                    }
                }
                if !write_stdout(&buf) {
                    return ExitCode::SUCCESS; // the client went away
                }
            }
            _ => {
                if state.stdin_closed.load(Ordering::SeqCst) {
                    return ExitCode::SUCCESS;
                }
                match reconnect(&state, opts, home) {
                    Some(new_reader) => reader = new_reader,
                    None => return ExitCode::from(1),
                }
            }
        }
    }
}

/// The daemon went away while the client is still running. Answer what it
/// will never answer, then reconnect so the client barely notices.
fn reconnect(state: &Pump, opts: &Opts, home: &Path) -> Option<BufReader<TcpStream>> {
    // Park writers until a new connection exists, so requests sent meanwhile
    // wait instead of vanishing into the dead socket.
    {
        let mut w = state.writer.lock().unwrap();
        *w = (w.0 + 1, None);
    }
    let init = state.init.lock().unwrap().clone();
    let answered = state.init_answered.load(Ordering::SeqCst);
    let init_key = init.as_ref().and_then(|i| i.get("id")).map(Value::to_string);
    let lost: Vec<Value> = state.inflight.lock().unwrap().drain().filter(|(k, _)| answered || Some(k) != init_key.as_ref()).map(|(_, id)| id).collect();
    for id in lost {
        let err =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "the hangar daemon went away before answering; please retry"}});
        let mut line = serde_json::to_vec(&err).unwrap_or_default();
        line.push(b'\n');
        write_stdout(&line);
    }
    let Some(init) = init else {
        eprintln!("hangar: lost the daemon before initialize; exiting");
        return None;
    };
    // Initialized: resume quietly. Not yet answered: ask again as new.
    let conn = if answered { connect(opts, home, Some(&init)) } else { connect(opts, home, None) };
    match conn {
        Ok((mut stream, new_reader, welcome)) if !welcome.direct => {
            if !answered {
                state.inflight.lock().unwrap().insert(init_key.unwrap_or_default(), init["id"].clone());
                if write_json(&mut stream, &init).is_err() {
                    return None;
                }
            }
            let mut w = state.writer.lock().unwrap();
            *w = (w.0 + 1, Some(stream));
            eprintln!("hangar: reconnected to the daemon");
            Some(new_reader)
        }
        _ => {
            eprintln!("hangar: lost the daemon and could not reconnect; exiting");
            None
        }
    }
}

fn stdin_to_daemon(state: &Pump) {
    let stdin = std::io::stdin();
    let mut input = BufReader::with_capacity(64 * 1024, stdin.lock());
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // A request: remember it until its answer comes back.
        if let Some(h) = head(&buf)
            && let (Some(method), Some(id)) = (&h.method, &h.id)
        {
            if method == "initialize" {
                let mut init = state.init.lock().unwrap();
                if init.is_none() {
                    *init = serde_json::from_slice(&buf).ok();
                }
            }
            state.inflight.lock().unwrap().insert(id.to_string(), id.clone());
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
            if let Some(mut w) = writer.as_ref()
                && failed_gen != Some(*generation)
            {
                if w.write_all(line).is_ok() {
                    return;
                }
                failed_gen = Some(*generation);
            }
        }
        if Instant::now() >= deadline {
            return; // dropped; the reconnect path answers it with an error
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stops children from inheriting this process's stdin, stdout and stderr.
///
/// Windows gives a child every inheritable handle of its parent, not just the
/// ones passed as its stdio. A long-lived daemon started from here would keep
/// our client's pipes open, so the client would never see them close. Rust
/// duplicates handles it passes to children explicitly, so `Stdio::inherit`
/// still works afterwards.
pub fn keep_std_handles_private() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(id);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Starts `hangar daemon` detached from the client's process tree.
fn start_daemon(home: &Path) -> Result<(), String> {
    if std::env::var(ENV_NO_AUTOSTART).map(|v| v == "1").unwrap_or(false) {
        return Err("no daemon running and autostart is disabled".into());
    }
    keep_std_handles_private();
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
        // A crash (negative NTSTATUS, signal) must not look like success.
        Ok(status) => ExitCode::from(status.code().filter(|c| (0..=255).contains(c)).unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("hangar-shim: could not start {}: {e}", program.display());
            ExitCode::from(127)
        }
    }
}

/// Finds a bare command on this process's PATH the way a shell would,
/// including `.cmd` wrappers such as `npx.cmd` on Windows.
pub fn resolve_command(command: &str) -> Option<PathBuf> {
    resolve_command_in(command, std::env::var_os("PATH").as_deref(), std::env::var("PATHEXT").ok().as_deref())
}

/// Like [`resolve_command`], but searches the given PATH (for example the
/// environment of the client that asked for the server).
pub fn resolve_command_in(command: &str, path_var: Option<&std::ffi::OsStr>, pathext: Option<&str>) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.components().count() > 1 || path.is_absolute() {
        return Some(path.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        if path.extension().is_some() {
            vec![String::new()]
        } else {
            pathext.unwrap_or(".COM;.EXE;.BAT;.CMD").split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect()
        }
    } else {
        vec![String::new()]
    };
    let dirs = path_var?;
    for dir in std::env::split_paths(dirs) {
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
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
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
    fn parses_its_command_line() {
        let o = parse(&["--name", "pw", "--env-keys", "A, B", "--", "cmd", "/c", "npx", "-y", "@playwright/mcp@latest"]).unwrap();
        assert_eq!(o.name, "pw");
        assert_eq!(o.env_keys, vec!["A", "B"]);
        assert_eq!(o.command, "cmd");
        assert_eq!(o.args, vec!["/c", "npx", "-y", "@playwright/mcp@latest"]);
        // The separator is optional.
        let o = parse(&["node", "server.js", "--flag"]).unwrap();
        assert_eq!(o.command, "node");
        assert_eq!(o.args, vec!["server.js", "--flag"]);
        // What follows the command belongs to the server, even if it looks like an option.
        assert_eq!(parse(&["--", "npx", "--name", "x"]).unwrap().args, vec!["--name", "x"]);
        assert!(parse(&["--name", "x"]).is_err(), "a command is required");
        assert!(parse(&["--bogus", "x"]).is_err(), "unknown options are refused");
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
