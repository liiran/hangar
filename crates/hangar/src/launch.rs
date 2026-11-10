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

/// Reads a variable from a client environment, ignoring case on Windows
/// (where `Path` and `PATH` are the same variable). Falls back to our own.
fn env_var(env: &BTreeMap<String, String>, name: &str) -> Option<String> {
    let found = if cfg!(windows) { env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()) } else { env.get(name).cloned() };
    found.or_else(|| std::env::var(name).ok()).filter(|v| !v.is_empty())
}

/// Resolves `command` on the PATH of the environment the server will run in.
pub fn resolve(command: &str, env: &BTreeMap<String, String>) -> PathBuf {
    let path = env_var(env, "PATH");
    let pathext = env_var(env, "PATHEXT");
    hangar_shim::resolve_command_in(command, path.as_deref().map(std::ffi::OsStr::new), pathext.as_deref()).unwrap_or_else(|| PathBuf::from(command))
}

/// The plain launch: exactly what the client config says.
pub fn as_configured(spec: &ServerSpec, env: &BTreeMap<String, String>) -> Launch {
    Launch { program: resolve(&spec.command, env), args: spec.args.clone(), fast: false }
}

/// When the npx fast path may be used.
#[derive(Debug, Clone, Copy, Default)]
pub struct FastPath<'a> {
    pub enabled: bool,
    /// Where hangar remembers when real npx last ran each spec.
    pub state_dir: Option<&'a Path>,
}

/// The best launch for `spec`, using the npx fast path when it applies.
pub fn plan(spec: &ServerSpec, env: &BTreeMap<String, String>, fast: FastPath<'_>) -> Launch {
    if fast.enabled
        && let Some(l) = npx_fast_path(spec, env, fast.state_dir)
    {
        return l;
    }
    as_configured(spec, env)
}

/// How long a successful real `npx` run vouches for the cached copy of a
/// spec that names a tag (`@latest`) or no version.
pub const FRESH_FOR_MS: u64 = 24 * 60 * 60 * 1000;

/// Whether the spec pins an exact version, which npx never updates.
pub fn is_exact(pkg_spec: &str) -> bool {
    let after_name = pkg_spec.strip_prefix('@').map_or(pkg_spec, |s| s).split_once('@').map(|(_, v)| v);
    after_name.is_some_and(|v| {
        let core = v.split(['-', '+']).next().unwrap_or(v);
        let parts: Vec<&str> = core.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    })
}

/// The file in a state dir that records when real npx last resolved each package.
pub const FRESH_FILE: &str = "npx-fresh.json";

fn fresh_file(dir: &Path) -> PathBuf {
    dir.join(FRESH_FILE)
}

static FRESH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Remembers that real npx just started `spec` successfully (so its cache is current).
pub fn note_npx_ran(state_dir: Option<&Path>, spec: &ServerSpec) {
    let (Some(dir), Some((pkg, _))) = (state_dir, parse_npx(spec)) else { return };
    if is_exact(&pkg) {
        return;
    }
    let _guard = FRESH_LOCK.lock().unwrap();
    let path = fresh_file(dir);
    let mut map: serde_json::Map<String, Value> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    map.insert(pkg, Value::from(crate::util::now_ms()));
    let _ = crate::util::write_private(&path, &serde_json::to_vec_pretty(&map).unwrap_or_default());
}

fn recently_refreshed(state_dir: Option<&Path>, pkg: &str) -> bool {
    let Some(dir) = state_dir else { return false };
    let map: serde_json::Map<String, Value> = std::fs::read(fresh_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    map.get(pkg).and_then(Value::as_u64).is_some_and(|t| crate::util::now_ms().saturating_sub(t) < FRESH_FOR_MS)
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
    if cfg!(windows) { Some(Path::new(&env_var(env, "LOCALAPPDATA")?).join("npm-cache")) } else { Some(Path::new(&env_var(env, "HOME")?).join(".npm")) }
}

/// The folder npx installs `pkg_spec` into.
pub fn npx_dir(cache: &Path, pkg_spec: &str) -> PathBuf {
    let digest = Sha512::digest(pkg_spec.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    cache.join("_npx").join(hex)
}

/// npm package names: `name` or `@scope/name`, no path tricks.
fn valid_package_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('/').collect();
    let ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'));
    match parts.as_slice() {
        [n] => ok(n),
        [scope, n] => scope.starts_with('@') && ok(&scope[1..]) && ok(n),
        _ => false,
    }
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
    if !valid_package_name(&name) {
        return None;
    }
    let pkg_dir = npx_dir(cache, pkg_spec).join("node_modules").join(&name);
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(pkg_dir.join("package.json")).ok()?).ok()?;
    let rel = match manifest.get("bin")? {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            let unscoped = name.rsplit('/').next().unwrap_or(&name);
            if map.len() == 1 { map.values().next()?.as_str()?.to_string() } else { map.get(unscoped)?.as_str()?.to_string() }
        }
        _ => return None,
    };
    // The entry must live inside the package (no `../` escapes).
    let script = pkg_dir.join(rel).canonicalize().ok()?;
    let root = pkg_dir.canonicalize().ok()?;
    (script.starts_with(&root) && script.is_file()).then(|| strip_verbatim(script))
}

/// `canonicalize` on Windows returns `\\?\C:\...`; node and cmd prefer `C:\...`.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn npx_fast_path(spec: &ServerSpec, env: &BTreeMap<String, String>, state_dir: Option<&Path>) -> Option<Launch> {
    let (pkg, rest) = parse_npx(spec)?;
    // `npx -y pkg@latest` asks the registry for updates on every start. Only
    // skip that when real npx did it recently; exact versions never change.
    if !is_exact(&pkg) && !recently_refreshed(state_dir, &pkg) {
        return None;
    }
    let cache = npm_cache_dir(env)?;
    let script = cached_entry(&cache, &pkg)?;
    let node = resolve("node", env);
    if !node.is_file() {
        return None;
    }
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
        assert_eq!(
            parse_npx(&spec("cmd", &["/c", "npx", "-y", "@playwright/mcp@latest", "--headless"])),
            Some(("@playwright/mcp@latest".into(), vec!["--headless".into()]))
        );
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
    fn rejects_package_names_that_escape_the_cache() {
        assert!(valid_package_name("@playwright/mcp"));
        assert!(valid_package_name("chrome-devtools-mcp"));
        assert!(!valid_package_name("@x/../../evil"));
        assert!(!valid_package_name(".."));
        assert!(!valid_package_name("a/b/c"));
        assert!(!valid_package_name("@/x"));
        assert!(!valid_package_name(r"x\y"));
    }

    #[test]
    fn entries_outside_the_package_are_refused() {
        let cache = std::env::temp_dir().join(format!("hangar-npx-escape-{}", std::process::id()));
        let pkg_dir = npx_dir(&cache, "tool").join("node_modules").join("tool");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(cache.join("outside.js"), "").unwrap();
        std::fs::write(pkg_dir.join("package.json"), r#"{"bin":"../../../../outside.js"}"#).unwrap();
        assert_eq!(cached_entry(&cache, "tool"), None);
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn only_exact_versions_skip_the_freshness_check() {
        assert!(is_exact("@playwright/mcp@0.0.85"));
        assert!(is_exact("tool@1.2.3-beta.1"));
        assert!(!is_exact("@playwright/mcp@latest"));
        assert!(!is_exact("tool"));
        assert!(!is_exact("tool@^1.2.3"));
        assert!(!is_exact("@scope/tool"));
    }

    #[test]
    fn tagged_specs_use_the_fast_path_only_after_a_recent_npx_run() {
        let dir = std::env::temp_dir().join(format!("hangar-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = ServerSpec { command: "npx".into(), args: vec!["-y".into(), "tool@latest".into()], env: Default::default() };
        assert!(!recently_refreshed(Some(&dir), "tool@latest"));
        note_npx_ran(Some(&dir), &spec);
        assert!(recently_refreshed(Some(&dir), "tool@latest"));
        assert!(!recently_refreshed(None, "tool@latest"));
        let _ = std::fs::remove_dir_all(&dir);
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
        let expected = strip_verbatim(pkg_dir.join("dist/cli.js").canonicalize().unwrap());
        assert_eq!(cached_entry(&cache, "@scope/tool@latest"), Some(expected));
        assert_eq!(cached_entry(&cache, "@scope/tool@1.0.0"), None, "different spec, different cache dir");
        let _ = std::fs::remove_dir_all(&cache);
    }
}
