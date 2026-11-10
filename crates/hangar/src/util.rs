//! Small helpers shared across modules.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
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

const SECRET_WORDS: &[&str] = &["key", "token", "secret", "password", "passwd", "auth", "credential", "bearer"];

/// Whether a variable or flag name suggests it holds a credential.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|w| n.contains(w))
}

/// A command line for display, with likely secrets replaced by `***`:
/// the value after a flag such as `--api-key`, `NAME=value` pairs whose
/// name mentions a key or token, and URLs with embedded credentials.
pub fn redact_command(parts: &[String]) -> String {
    let mut out = Vec::with_capacity(parts.len());
    let mut hide_next = false;
    for p in parts {
        if hide_next {
            out.push("***".to_string());
            hide_next = false;
            continue;
        }
        if p.starts_with('-') {
            let (flag, value) = p.split_once('=').map_or((p.as_str(), None), |(f, v)| (f, Some(v)));
            if looks_secret(flag) {
                match value {
                    Some(_) => out.push(format!("{flag}=***")),
                    None => {
                        out.push(p.clone());
                        hide_next = true;
                    }
                }
                continue;
            }
        } else if let Some((name, _)) = p.split_once('=')
            && looks_secret(name)
            && !name.contains('/')
        {
            out.push(format!("{name}=***"));
            continue;
        }
        if let Some(at) = p.find('@')
            && let Some(scheme) = p.find("://")
            && scheme < at
            && p[scheme + 3..at].contains(':')
        {
            out.push(format!("{}://***@{}", &p[..scheme], &p[at + 1..]));
            continue;
        }
        out.push(p.clone());
    }
    out.iter().map(|s| if s.contains(' ') { format!("\"{s}\"") } else { s.clone() }).collect::<Vec<_>>().join(" ")
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
