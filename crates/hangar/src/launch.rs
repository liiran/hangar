//! How a server process is actually started.
//!
//! Most MCP servers are configured as `npx -y <package>` (on Windows,
//! `cmd /c npx -y <package>`). That launches a chain of processes: `cmd`,
//! a Node process running npm's `npx-cli.js` (about 70 MB that does nothing
//! after startup), another `cmd`, and finally the server's own Node process.
//! It also asks the npm registry for updates on every start.
//!
//! When the package is already in npx's cache, the npx fast path starts the
//! package's entry script with `node` directly. npx names its cache folder
//! with the first 16 hex digits of SHA-512 of the package spec, so this needs
//! no network and no guessing. If anything about the command is unusual, or
//! the fast start fails, hangar uses the configured command unchanged.

use hangar_proto::ServerSpec;
use serde_json::Value;
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Started through the npx fast path (fall back to the spec if it fails).
    pub fast: bool,
}

impl Launch {
    pub fn display(&self) -> String {
        std::iter::once(self.program.display().to_string()).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

/// The plain launch: exactly what the client config says.
pub fn as_configured(spec: &ServerSpec) -> Launch {
    let program = hangar_shim::resolve_command(&spec.command).unwrap_or_else(|| PathBuf::from(&spec.command));
    Launch { program, args: spec.args.clone(), fast: false }
}

/// The best launch for `spec`, using the npx fast path when it applies.
pub fn plan(spec: &ServerSpec, env: &BTreeMap<String, String>, fast_npx: bool) -> Launch {
    if fast_npx {
        if let Some(l) = npx_fast_path(spec, env) {
            return l;
        }
    }
    as_configured(spec)
}

fn base(token: &str) -> String {
    let b = token.rsplit(['/', '\\']).next().unwrap_or(token).to_ascii_lowercase();
    b.trim_end_matches(".exe").trim_end_matches(".cmd").to_string()
}

/// Splits `[cmd /c] npx [-y] <pkg> [args]` into (pkg spec, server args).
pub fn parse_npx(spec: &ServerSpec) -> Option<(String, Vec<String>)> {
    let mut tokens: Vec<String> = std::iter::once(spec.command.clone()).chain(spec.args.iter().cloned()).collect();
    if base(&tokens[0]) == "cmd" {
        // cmd [/d] [/s] /c npx ...
        let at = tokens.iter().position(|t| t.eq_ignore_ascii_case("/c"))?;
        if tokens[1..at].iter().any(|t| !t.eq_ignore_ascii_case("/d") && !t.eq_ignore_ascii_case("/s")) {
            return None;
        }
        tokens.drain(..=at);
    }
    if tokens.is_empty() || base(&tokens[0]) != "npx" {
        return None;
    }
    let mut i = 1;
    while i < tokens.len() && tokens[i].starts_with('-') {
        match tokens[i].as_str() {
            "-y" | "--yes" | "-q" | "--quiet" => i += 1,
            _ => return None, // --package, -c, --node-options...: leave it to npx
        }
    }
    let pkg = tokens.get(i)?.clone();
    // Local paths, tarballs, git URLs and shell syntax are npx's business.
    if pkg.starts_with('.') || pkg.contains(['\\', ':', '&', '|', '>', '<', '^', '%']) || pkg.ends_with(".tgz") || pkg.ends_with(".js") {
        return None;
    }
    if tokens[i + 1..].iter().any(|t| t.contains(['&', '|', '>', '<', '^', '%'])) && base(&spec.command) == "cmd" {
        return None; // cmd would have interpreted these
    }
    Some((pkg, tokens[i + 1..].to_vec()))
}

/// npm's cache folder.
pub fn npm_cache_dir(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    for key in ["npm_config_cache", "NPM_CONFIG_CACHE"] {
        if let Some(v) = env.get(key).cloned().or_else(|| std::env::var(key).ok()).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(v));
        }
    }
    if cfg!(windows) {
        let local = env.get("LOCALAPPDATA").cloned().or_else(|| std::env::var("LOCALAPPDATA").ok())?;
        Some(Path::new(&local).join("npm-cache"))
    } else {
        let home = env.get("HOME").cloned().or_else(|| std::env::var("HOME").ok())?;
        Some(Path::new(&home).join(".npm"))
    }
}

/// The folder npx installs `pkg_spec` into.
pub fn npx_dir(cache: &Path, pkg_spec: &str) -> PathBuf {
    let digest = Sha512::digest(pkg_spec.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    cache.join("_npx").join(hex)
}

fn package_name(spec: &str) -> String {
    if let Some(scoped) = spec.strip_prefix('@') {
        format!("@{}", scoped.split('@').next().unwrap_or(scoped))
    } else {
        spec.split('@').next().unwrap_or(spec).to_string()
    }
}

/// The entry script npx would run for `pkg_spec`, if it is cached.
pub fn cached_entry(cache: &Path, pkg_spec: &str) -> Option<PathBuf> {
    let name = package_name(pkg_spec);
    let pkg_dir = npx_dir(cache, pkg_spec).join("node_modules").join(&name);
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(pkg_dir.join("package.json")).ok()?).ok()?;
    let rel = match manifest.get("bin")? {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            let unscoped = name.rsplit('/').next().unwrap_or(&name);
            if map.len() == 1 {
                map.values().next()?.as_str()?.to_string()
            } else {
                map.get(unscoped)?.as_str()?.to_string()
            }
        }
        _ => return None,
    };
    let script = pkg_dir.join(rel);
    script.is_file().then_some(script)
}

fn npx_fast_path(spec: &ServerSpec, env: &BTreeMap<String, String>) -> Option<Launch> {
    let (pkg, rest) = parse_npx(spec)?;
    let cache = npm_cache_dir(env)?;
    let script = cached_entry(&cache, &pkg)?;
    let node = hangar_shim::resolve_command("node")?;
    let mut args = vec![script.display().to_string()];
    args.extend(rest);
    Some(Launch { program: node, args, fast: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cmd: &str, args: &[&str]) -> ServerSpec {
        ServerSpec { command: cmd.into(), args: args.iter().map(|s| s.to_string()).collect(), env: Default::default() }
    }

    #[test]
    fn parses_npx_commands() {
        assert_eq!(parse_npx(&spec("cmd", &["/c", "npx", "-y", "@playwright/mcp@latest", "--headless"])), Some(("@playwright/mcp@latest".into(), vec!["--headless".into()])));
        assert_eq!(parse_npx(&spec("npx", &["-y", "pkg"])), Some(("pkg".into(), vec![])));
        assert_eq!(parse_npx(&spec("C:/node/npx.cmd", &["pkg", "a"])), Some(("pkg".into(), vec!["a".into()])));
        assert_eq!(parse_npx(&spec("cmd", &["/d", "/s", "/c", "npx", "pkg"])).map(|p| p.0), Some("pkg".into()));
        assert_eq!(parse_npx(&spec("npx", &["--package", "x", "y"])), None);
        assert_eq!(parse_npx(&spec("npx", &["./local.js"])), None);
        assert_eq!(parse_npx(&spec("cmd", &["/c", "npx", "pkg", "a&b"])), None);
        assert_eq!(parse_npx(&spec("node", &["server.js"])), None);
        assert_eq!(parse_npx(&spec("cmd", &["/k", "npx", "pkg"])), None);
    }

    #[test]
    fn npx_dir_matches_npm() {
        // Verified against real npm caches: sha512(spec)[..16].
        let d = npx_dir(Path::new("C:/cache"), "@modelcontextprotocol/server-memory");
        assert!(d.ends_with("_npx/15b07286cbcc3329") || d.ends_with("_npx\\15b07286cbcc3329"), "{}", d.display());
        assert!(npx_dir(Path::new("x"), "@playwright/mcp@latest").ends_with("9833c18b2d85bc59"));
    }

    #[test]
    fn package_names_drop_versions() {
        assert_eq!(package_name("@playwright/mcp@latest"), "@playwright/mcp");
        assert_eq!(package_name("chrome-devtools-mcp@1.2.3"), "chrome-devtools-mcp");
        assert_eq!(package_name("plain"), "plain");
    }

    #[test]
    fn finds_cached_entry_scripts() {
        let cache = std::env::temp_dir().join(format!("hangar-npx-{}", std::process::id()));
        let pkg_dir = npx_dir(&cache, "@scope/tool@latest").join("node_modules").join("@scope/tool");
        std::fs::create_dir_all(pkg_dir.join("dist")).unwrap();
        std::fs::write(pkg_dir.join("dist/cli.js"), "").unwrap();
        std::fs::write(pkg_dir.join("package.json"), r#"{"name":"@scope/tool","bin":{"tool":"dist/cli.js","other":"x.js"}}"#).unwrap();
        assert_eq!(cached_entry(&cache, "@scope/tool@latest"), Some(pkg_dir.join("dist/cli.js")));
        assert_eq!(cached_entry(&cache, "@scope/tool@1.0.0"), None, "different spec, different cache dir");
        let _ = std::fs::remove_dir_all(&cache);
    }
}
