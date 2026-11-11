//! Small helpers shared across modules.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
/// The user's home folder (`USERPROFILE` on Windows, `HOME` elsewhere).
pub fn user_home() -> std::path::PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from).unwrap_or_default()
}

/// Locks a mutex even if a thread panicked while holding it, so one bad
/// request cannot make every later one fail. hangar's shared state is
/// updated in small steps that leave it usable at every point.
pub trait Locked<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> Locked<T> for std::sync::Mutex<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `1.5 GB`, `340 MB`.
pub fn fmt_bytes(b: u64) -> String {
    let mb = b as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 { format!("{:.2} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

/// `n` random bytes from the OS, as lowercase hex.
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("OS randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares secrets without leaking where they differ through timing.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const SECRET_WORDS: &[&str] = &["key", "token", "secret", "password", "passwd", "auth", "credential", "bearer", "cookie", "signature"];
/// Short names that only count as a whole word (`GITHUB_PAT`, not `PATH`).
const SECRET_NAMES: &[&str] = &["pat", "pass", "pwd", "dsn", "sas", "jwt", "otp", "pin"];
/// Names that say which account, server or data a program works with.
const IDENTITY_NAMES: &[&str] = &[
    "url",
    "uri",
    "host",
    "endpoint",
    "profile",
    "kubeconfig",
    "config",
    "account",
    "tenant",
    "org",
    "organization",
    "project",
    "workspace",
    "region",
    "user",
    "username",
    "login",
    "email",
    "headers",
    "header",
    "database",
    "db",
    "file",
    "path",
    "dir",
    "home",
    "root",
];

/// The words of a variable or flag name: `--api-key` and `API_KEY` are both `api`, `key`.
fn name_words(name: &str) -> Vec<String> {
    name.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Whether a variable or flag name suggests it holds a credential.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|w| n.contains(w)) || name_words(name).iter().any(|w| SECRET_NAMES.contains(&w.as_str()))
}

/// Whether two sessions with different values for this variable should get
/// separate servers: it holds a credential, or picks an account, a server
/// or a data location (`AWS_PROFILE`, `DATABASE_URL`, `MEMORY_FILE_PATH`).
/// `PATH` and `HOME`-like variables every program has do not count.
pub fn separates_sessions(name: &str) -> bool {
    const COMMON: &[&str] = &[
        "PATH",
        "PATHEXT",
        "HOME",
        "HOMEPATH",
        "HOMEDRIVE",
        "USERPROFILE",
        "USERNAME",
        "USER",
        "LOGNAME",
        "USERDOMAIN",
        "USERDOMAIN_ROAMINGPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
        "COMMONPROGRAMW6432",
        "ALLUSERSPROFILE",
        "PUBLIC",
        "SYSTEMDRIVE",
        "COMSPEC",
        "PWD",
        "OLDPWD",
        "SHELL",
        "PSMODULEPATH",
        "ONEDRIVE",
        "DRIVERDATA",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_DIRS",
        "XDG_CONFIG_DIRS",
        "MANPATH",
        "INFOPATH",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "EXEPATH",
        "ORIGINAL_PATH",
        "MSYSTEM_PREFIX",
        "PKG_CONFIG_PATH",
        "ACLOCAL_PATH",
        "NVM_DIR",
        "NVM_HOME",
        "NVM_SYMLINK",
        "PNPM_HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
        "GOROOT",
        "JAVA_HOME",
        "ANDROID_HOME",
        "CONDA_PREFIX",
        "VIRTUAL_ENV",
        "PYENV_ROOT",
    ];
    if looks_secret(name) {
        return true;
    }
    !COMMON.contains(&name.to_ascii_uppercase().as_str()) && name_words(name).iter().any(|w| IDENTITY_NAMES.contains(&w.as_str()))
}

/// A URL for display: a password, a bare token before `@`, and the values
/// of query parameters whose names look secret become `***`.
fn redact_url(p: &str) -> Option<String> {
    let scheme = p.find("://")?;
    let (head, rest) = p.split_at(scheme + 3);
    let (rest, changed_user) = match rest.find('@').filter(|at| !rest[..*at].contains('/')) {
        Some(at) => (format!("***@{}", &rest[at + 1..]), true),
        None => (rest.to_string(), false),
    };
    let Some((base, query)) = rest.split_once('?') else { return changed_user.then(|| format!("{head}{rest}")) };
    let mut changed = changed_user;
    let query: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if looks_secret(name) => {
                changed = true;
                format!("{name}=***")
            }
            _ => pair.to_string(),
        })
        .collect();
    changed.then(|| format!("{head}{base}?{}", query.join("&")))
}

/// A command line for display, with likely secrets replaced by `***`:
/// the value after a flag such as `--api-key` or `--header`, `NAME=value`
/// pairs whose name mentions a key or token, anything after `Bearer`, and
/// credentials inside URLs.
pub fn redact_command(parts: &[String]) -> String {
    let mut out = Vec::with_capacity(parts.len());
    let mut hide_next = false;
    let pair = |p: &str| p.split_once('=').filter(|(name, _)| looks_secret(name) && !name.contains('/')).map(|(name, _)| format!("{name}=***"));
    for p in parts {
        if hide_next {
            out.push("***".to_string());
            hide_next = false;
            continue;
        }
        if p.to_ascii_lowercase().contains("bearer ") || p.to_ascii_lowercase().starts_with("authorization:") {
            out.push("***".to_string());
            continue;
        }
        if p.starts_with('-') {
            let (flag, value) = p.split_once('=').map_or((p.as_str(), None), |(f, v)| (f, Some(v)));
            // Headers nearly always carry credentials.
            let header = matches!(flag, "-H" | "--header" | "--headers");
            if looks_secret(flag) || header {
                match value {
                    Some(_) => out.push(format!("{flag}=***")),
                    None => {
                        out.push(p.clone());
                        hide_next = true;
                    }
                }
                continue;
            }
            // `--env=NAME=value`, `-eNAME=value`
            if let Some(hidden) = value.and_then(pair) {
                out.push(format!("{flag}={hidden}"));
                continue;
            }
        } else if let Some(hidden) = pair(p) {
            out.push(hidden);
            continue;
        }
        out.push(redact_url(p).unwrap_or_else(|| p.clone()));
    }
    out.iter().map(|s| if s.contains(' ') { format!("\"{s}\"") } else { s.clone() }).collect::<Vec<_>>().join(" ")
}

/// Text from another program, safe to print in a terminal: control
/// characters (which could move the cursor or rewrite lines) are dropped.
pub fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\t').collect()
}

/// Writes a file only the current user can read (Unix: mode 0600 from
/// creation; Windows: inherits the user's profile ACL), atomically.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path)
}

/// Appends a line to a file only the current user can read.
pub fn append_private(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    let mut f = opts.open(path)?;
    writeln!(f, "{line}")
}

/// Restricts a file or folder to the current user (Unix; Windows profile
/// folders already are).
pub fn make_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn redacts_secrets_but_keeps_the_command_readable() {
        assert_eq!(redact_command(&s(&["npx", "-y", "tavily-mcp", "--api-key", "tvly-123"])), "npx -y tavily-mcp --api-key ***");
        assert_eq!(redact_command(&s(&["srv", "--token=abc", "--port", "80"])), "srv --token=*** --port 80");
        assert_eq!(redact_command(&s(&["env", "GITHUB_TOKEN=ghp_x", "node", "a.js"])), "env GITHUB_TOKEN=*** node a.js");
        assert_eq!(redact_command(&s(&["npx", "mcp-remote", "https://host/mcp?apiKey=SECRET&v=2"])), "npx mcp-remote https://host/mcp?apiKey=***&v=2");
        assert_eq!(redact_command(&s(&["git", "clone", "https://ghp_abc@github.com/o/r"])), "git clone https://***@github.com/o/r");
        assert_eq!(redact_command(&s(&["srv", "--header", "Authorization: Bearer abc", "-H", "X-Api: 1"])), "srv --header *** -H ***");
        assert_eq!(redact_command(&s(&["docker", "run", "--env=API_KEY=abc", "-e", "DB_PASS=x", "img"])), "docker run --env=API_KEY=*** -e DB_PASS=*** img");
        assert_eq!(redact_command(&s(&["srv", "Bearer abc.def"])), "srv ***");
        assert_eq!(
            redact_command(&s(&["npx", "-y", "@scope/pkg@1.2.3", "https://example.com/a?b=1"])),
            "npx -y @scope/pkg@1.2.3 https://example.com/a?b=1",
            "ordinary arguments are untouched"
        );
    }

    #[test]
    fn knows_which_variables_tell_sessions_apart() {
        for name in [
            "GITHUB_PAT",
            "DB_PASS",
            "PGPASSWORD",
            "AWS_PROFILE",
            "DATABASE_URL",
            "MONGODB_URI",
            "KUBECONFIG",
            "OPENAPI_MCP_HEADERS",
            "MEMORY_FILE_PATH",
            "AZURE_CONFIG_DIR",
            "ANTHROPIC_BASE_URL",
        ] {
            assert!(separates_sessions(name), "{name}");
        }
        for name in [
            "PATH",
            "Path",
            "HOME",
            "USERPROFILE",
            "TEMP",
            "LANG",
            "NUMBER_OF_PROCESSORS",
            "TERM",
            "COLORTERM",
            "PROCESSOR_ARCHITECTURE",
            "NVM_HOME",
            "ComSpec",
        ] {
            assert!(!separates_sessions(name), "{name}");
        }
        assert!(looks_secret("GITHUB_PAT") && !looks_secret("PATH") && !looks_secret("COMPATIBLE"));
        assert_eq!(printable("ok\x1b[2Jgone\r\n\tx"), "ok[2Jgone\tx");
        assert_eq!(redact_command(&s(&["pg", "postgres://user:pw@db:5432/x"])), "pg postgres://***@db:5432/x");
        assert_eq!(redact_command(&s(&["npx", "@scope/pkg@latest", "--headless"])), "npx @scope/pkg@latest --headless");
        assert_eq!(redact_command(&s(&["node", "C:/My Dir/a.js"])), "node \"C:/My Dir/a.js\"");
    }

    #[test]
    fn constant_time_eq_compares_whole_strings() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(fmt_bytes(340 << 20), "340 MB");
        assert_eq!(fmt_bytes(3 << 29), "1.50 GB");
        assert_eq!(random_hex(4).len(), 8);
    }
}
