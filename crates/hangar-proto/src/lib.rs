//! Wire types shared by the hangar daemon and `hangar-shim`.
//!
//! A shim connects to the daemon over loopback TCP and sends one [`Hello`]
//! line. The daemon answers with one [`Welcome`] line. After that the socket
//! carries plain newline-delimited JSON-RPC, exactly like MCP's stdio framing.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bumped whenever [`Hello`] or [`Welcome`] change incompatibly.
pub const WIRE_VERSION: u32 = 1;

/// Overrides the state directory (default `~/.hangar`).
pub const ENV_HOME: &str = "HANGAR_HOME";
/// Overrides the path of the `hangar` binary the shim starts on demand.
pub const ENV_BIN: &str = "HANGAR_BIN";
/// Set to `1` to stop the shim from starting a daemon by itself.
pub const ENV_NO_AUTOSTART: &str = "HANGAR_NO_AUTOSTART";
/// Set by the session manager so pooled servers know which session they serve.
pub const ENV_SESSION: &str = "HANGAR_SESSION";

/// How to start an MCP server: the same fields an MCP client config uses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables declared in the client config. They are part of
    /// the server's identity, so two configs that differ here never share.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl ServerSpec {
    /// A stable fingerprint of the spec. Equal specs always share a fingerprint.
    pub fn fingerprint(&self) -> String {
        let mut normalized = self.clone();
        if cfg!(windows) {
            normalized.command = normalized.command.to_ascii_lowercase();
        }
        let text = serde_json::to_string(&normalized).unwrap_or_default();
        format!("{:016x}", fnv1a64(text.as_bytes()))
    }

    /// The command line as one human-readable string.
    pub fn display(&self) -> String {
        let mut out = self.command.clone();
        for arg in &self.args {
            out.push(' ');
            if arg.contains(' ') {
                out.push('"');
                out.push_str(arg);
                out.push('"');
            } else {
                out.push_str(arg);
            }
        }
        out
    }
}

/// First line any client sends: a random nonce the daemon must sign with
/// the token, so clients never reveal secrets to an impostor listening on a
/// stale port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Challenge {
    pub hangar: u32,
    pub challenge: String,
}

/// The daemon's answer to a [`Challenge`]: [`proof`] of the token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub proof: String,
}

/// SHA-256 over the token and the client's nonce, as hex.
pub fn proof(token: &str, nonce: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("hangar-proof:{token}:{nonce}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 32 random hex characters from the OS.
pub fn nonce() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Compares secrets without leaking where they differ through timing.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Second line a shim sends, once the daemon proved itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Must equal [`WIRE_VERSION`].
    pub hangar: u32,
    /// Copied from `daemon.json`; proves the shim runs as the daemon's user.
    pub token: String,
    /// The server's name in the client config, e.g. `playwright`.
    pub name: String,
    pub spec: ServerSpec,
    /// Working directory the client started the server in.
    pub cwd: String,
    /// The shim's full environment. Used to start the server, never to key it.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Session id when the session was started by hangar.
    #[serde(default)]
    pub session: Option<String>,
    /// Process id of the MCP client (the shim's parent).
    #[serde(default)]
    pub client_pid: Option<u32>,
    pub shim_pid: u32,
    /// Set when reconnecting: the `initialize` request the client already sent.
    /// The daemon attaches without answering it again.
    #[serde(default)]
    pub resume: Option<serde_json::Value>,
}

/// The daemon's answer to a [`Hello`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Welcome {
    pub ok: bool,
    /// The daemon declines to pool this server; the shim runs it itself.
    #[serde(default)]
    pub direct: bool,
    /// How the server is shared: `global`, `project` or `session`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Written by a running daemon to `<home>/daemon.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub pid: u32,
    pub shim_port: u16,
    #[serde(default)]
    pub ui_port: Option<u16>,
    pub token: String,
    pub version: String,
    pub started_at_ms: u64,
}

/// The state directory: `$HANGAR_HOME`, else `~/.hangar`.
pub fn home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_HOME).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join(".hangar")
}

pub fn daemon_info_path(home: &Path) -> PathBuf {
    home.join("daemon.json")
}

pub fn read_daemon_info(home: &Path) -> Option<DaemonInfo> {
    let text = std::fs::read_to_string(daemon_info_path(home)).ok()?;
    serde_json::from_str(&text).ok()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = Fnv::new();
    h.write(bytes);
    h.finish()
}

/// FNV-1a: a fast, stable, non-cryptographic hash for fingerprints.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Fnv::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cmd: &str, args: &[&str]) -> ServerSpec {
        ServerSpec { command: cmd.into(), args: args.iter().map(|s| s.to_string()).collect(), env: BTreeMap::new() }
    }

    #[test]
    fn fingerprint_is_stable_and_discriminating() {
        let a = spec("npx", &["-y", "@playwright/mcp@latest"]);
        assert_eq!(a.fingerprint(), a.clone().fingerprint());
        assert_ne!(a.fingerprint(), spec("npx", &["-y", "@playwright/mcp@0.0.1"]).fingerprint());
        let mut with_env = a.clone();
        with_env.env.insert("TOKEN".into(), "x".into());
        assert_ne!(a.fingerprint(), with_env.fingerprint());
    }

    #[test]
    fn proofs_depend_on_token_and_nonce() {
        let p = proof("tok", "n1");
        assert_eq!(p.len(), 64);
        assert_eq!(p, proof("tok", "n1"));
        assert_ne!(p, proof("tok", "n2"));
        assert_ne!(p, proof("other", "n1"));
        assert_ne!(nonce(), nonce());
        assert!(constant_time_eq("abc", "abc") && !constant_time_eq("abc", "abd") && !constant_time_eq("a", "ab"));
    }

    #[test]
    fn display_quotes_spaces() {
        assert_eq!(spec("node", &["C:/My Dir/s.js", "-v"]).display(), "node \"C:/My Dir/s.js\" -v");
    }
}
