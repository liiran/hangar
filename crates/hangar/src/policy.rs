//! Which servers can be shared, and between whom.
//!
//! Hangar never changes what a server returns. A server is shared only when
//! every session would have seen the same behavior from its own copy. That
//! depends on the server, so each one gets a [`Share`] level:
//!
//! * `global`: one process for every session (stateless API clients, docs).
//! * `project`: one process per working directory.
//! * `native`: one process that serves every session over HTTP with its own
//!   per-session state (Playwright with `--isolated`: one browser, a
//!   separate browser context per session).
//! * `session`: one process per session, as today, but still supervised:
//!   started lazily, restarted on crash, and never orphaned.
//! * `off`: not managed; the shim runs the server itself.
//!
//! Known servers come from a built-in registry. Unknown servers default to
//! `session`: supervised, but never shared, because hangar cannot know
//! whether they keep per-client state. Anything can be overridden in
//! `config.toml`.

use crate::config::Config;
use hangar_proto::ServerSpec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Share {
    Global,
    Project,
    Native,
    Session,
    Off,
}

impl Share {
    pub fn as_str(self) -> &'static str {
        match self {
            Share::Global => "global",
            Share::Project => "project",
            Share::Native => "native",
            Share::Session => "session",
            Share::Off => "off",
        }
    }

    /// Whether several sessions can be attached to one process.
    pub fn is_shared(self) -> bool {
        matches!(self, Share::Global | Share::Project)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub share: Share,
    pub package: Option<String>,
    pub reason: String,
    /// For `native`: the arguments that switch the server to HTTP mode.
    pub http_args: Option<Vec<String>>,
}

impl Decision {
    fn new(share: Share, package: Option<String>, reason: impl Into<String>) -> Decision {
        Decision { share, package, reason: reason.into(), http_args: None }
    }
}

/// Built-in knowledge about popular servers: (package, share, reason).
const REGISTRY: &[(&str, Share, &str)] = &[
    ("@playwright/mcp", Share::Session, "drives a browser; each session needs its own pages"),
    ("@upstash/context7-mcp", Share::Global, "stateless documentation lookup"),
    ("@modelcontextprotocol/server-memory", Share::Global, "one knowledge-graph file for everyone; sharing also avoids lost writes"),
    ("@modelcontextprotocol/server-sequential-thinking", Share::Session, "keeps each session's thought history in memory"),
    ("@modelcontextprotocol/server-filesystem", Share::Session, "asks each client for its workspace roots"),
    ("@modelcontextprotocol/server-everything", Share::Session, "test server with per-client features"),
    ("@modelcontextprotocol/server-puppeteer", Share::Session, "drives a browser"),
    ("@modelcontextprotocol/server-github", Share::Global, "stateless API client"),
    ("@modelcontextprotocol/server-gitlab", Share::Global, "stateless API client"),
    ("@modelcontextprotocol/server-brave-search", Share::Global, "stateless search API client"),
    ("@modelcontextprotocol/server-postgres", Share::Global, "read-only queries against the configured database"),
    ("@modelcontextprotocol/server-slack", Share::Global, "stateless API client"),
    ("@modelcontextprotocol/server-google-maps", Share::Global, "stateless API client"),
    ("chrome-devtools-mcp", Share::Session, "drives a browser"),
    ("@browsermcp/mcp", Share::Session, "drives a browser"),
    ("@wonderwhy-er/desktop-commander", Share::Session, "keeps terminal sessions per client"),
    ("@notionhq/notion-mcp-server", Share::Global, "stateless API client"),
    ("@sentry/mcp-server", Share::Global, "stateless API client"),
    ("@supabase/mcp-server-supabase", Share::Global, "stateless API client"),
    ("@stripe/mcp", Share::Global, "stateless API client"),
    ("@heroku/mcp-server", Share::Global, "stateless API client"),
    ("@azure/mcp", Share::Global, "stateless API client"),
    ("figma-developer-mcp", Share::Global, "stateless API client"),
    ("tavily-mcp", Share::Global, "stateless search API client"),
    ("exa-mcp-server", Share::Global, "stateless search API client"),
    ("firecrawl-mcp", Share::Global, "stateless scraping API client"),
    ("@perplexity-ai/mcp-server", Share::Global, "stateless search API client"),
    ("mcp-server-fetch", Share::Global, "stateless HTTP fetch"),
    ("mcp-server-time", Share::Global, "stateless clock and time zones"),
    ("mcp-server-git", Share::Project, "works on the repository it was started in"),
    ("mcp-server-sqlite", Share::Project, "opens a database relative to its folder"),
    ("serena", Share::Session, "keeps language-server and project state"),
];

const LAUNCHERS: &[&str] = &["npx", "pnpx", "bunx", "uvx", "pipx", "dlx"];

/// The npm or PyPI package a launcher command runs, without its version.
pub fn package_of(spec: &ServerSpec) -> Option<String> {
    let tokens: Vec<&str> = std::iter::once(spec.command.as_str()).chain(spec.args.iter().map(String::as_str)).collect();
    let base = |t: &str| {
        let b = t.rsplit(['/', '\\']).next().unwrap_or(t).to_ascii_lowercase();
        b.trim_end_matches(".cmd").trim_end_matches(".exe").to_string()
    };
    let at = tokens.iter().position(|t| LAUNCHERS.contains(&base(t).as_str()))?;
    let mut rest = tokens[at + 1..].iter();
    // `pipx run pkg`, `pnpm dlx pkg`
    let mut pkg = None;
    for t in rest.by_ref() {
        if t.starts_with('-') || *t == "run" {
            continue;
        }
        pkg = Some(*t);
        break;
    }
    let pkg = pkg?;
    Some(strip_version(pkg))
}

fn strip_version(pkg: &str) -> String {
    let pkg = pkg.split("==").next().unwrap_or(pkg);
    if let Some(scoped) = pkg.strip_prefix('@') {
        let name = scoped.split('@').next().unwrap_or(scoped);
        format!("@{name}")
    } else {
        pkg.split('@').next().unwrap_or(pkg).to_string()
    }
}

/// Decides how `spec` (configured as `name`) is shared.
pub fn decide(spec: &ServerSpec, name: &str, config: &Config) -> Decision {
    let package = package_of(spec);
    let builtin_http = crate::pool::http::native_args(package.as_deref(), spec);
    let overridden = config.servers.get(name).or_else(|| package.as_ref().and_then(|p| config.servers.get(p)));
    if let Some(o) = overridden.filter(|o| o.share.is_some()) {
        let share = o.share.unwrap();
        if share != Share::Native {
            return Decision::new(share, package, "set in config.toml");
        }
        let http_args = o.http_args.clone().or(builtin_http);
        return match http_args {
            Some(args) if args.iter().any(|a| a.contains("{port}")) => {
                Decision { http_args: Some(args), ..Decision::new(Share::Native, package, "set in config.toml") }
            }
            _ => Decision::new(Share::Session, package, "share = \"native\" needs http_args with a {port} placeholder; running one per session instead"),
        };
    }
    if let Some(pkg) = &package {
        if pkg == "@playwright/mcp" {
            // One shared browser means Playwright listens on a local port
            // that has no password, which any program on this machine could
            // use. That is the user's call to make, so it is off by default.
            let reason = if builtin_http.is_some() {
                "one per session; share = \"native\" in config.toml runs one shared browser (only on a PC you do not share)"
            } else {
                "without --isolated, Playwright keeps one persistent profile that only one session can use"
            };
            return Decision::new(Share::Session, package, reason);
        }
        if let Some((_, share, reason)) = REGISTRY.iter().find(|(p, _, _)| p == pkg) {
            return Decision::new(*share, package, *reason);
        }
    }
    // Unknown servers may keep state per client, so they are not shared.
    Decision::new(Share::Session, package, "unknown server: one per session (mark it shareable in config.toml)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerOverride;

    fn spec(cmd: &str, args: &[&str]) -> ServerSpec {
        ServerSpec { command: cmd.into(), args: args.iter().map(|s| s.to_string()).collect(), env: Default::default() }
    }

    #[test]
    fn finds_packages_behind_launchers() {
        assert_eq!(package_of(&spec("cmd", &["/c", "npx", "-y", "@playwright/mcp@latest"])).as_deref(), Some("@playwright/mcp"));
        assert_eq!(package_of(&spec("npx", &["-y", "chrome-devtools-mcp@0.9.0", "--headless"])).as_deref(), Some("chrome-devtools-mcp"));
        assert_eq!(package_of(&spec("C:/bin/npx.cmd", &["@upstash/context7-mcp"])).as_deref(), Some("@upstash/context7-mcp"));
        assert_eq!(package_of(&spec("uvx", &["mcp-server-fetch==2025.1.0"])).as_deref(), Some("mcp-server-fetch"));
        assert_eq!(package_of(&spec("pipx", &["run", "serena"])).as_deref(), Some("serena"));
        assert_eq!(package_of(&spec("node", &["server.js"])), None);
    }

    #[test]
    fn registry_and_defaults() {
        let c = Config::default();
        assert_eq!(decide(&spec("npx", &["-y", "@upstash/context7-mcp@latest"]), "context7", &c).share, Share::Global);
        assert_eq!(decide(&spec("npx", &["-y", "@playwright/mcp"]), "pw", &c).share, Share::Session);
        assert_eq!(
            decide(&spec("npx", &["-y", "@playwright/mcp", "--isolated"]), "pw", &c).share,
            Share::Session,
            "a shared browser opens a local port, so it is opt-in"
        );
        assert_eq!(decide(&spec("node", &["mine.js"]), "mine", &c).share, Share::Session, "unknown servers are never shared by default");
    }

    #[test]
    fn config_overrides_by_name_or_package() {
        let mut c = Config::default();
        c.servers.insert("pw".into(), ServerOverride { share: Some(Share::Off), http_args: None });
        c.servers.insert("@upstash/context7-mcp".into(), ServerOverride { share: Some(Share::Session), http_args: None });
        assert_eq!(decide(&spec("npx", &["@playwright/mcp"]), "pw", &c).share, Share::Off);
        assert_eq!(decide(&spec("npx", &["@upstash/context7-mcp"]), "docs", &c).share, Share::Session);
    }

    #[test]
    fn native_needs_http_arguments() {
        let mut c = Config::default();
        c.servers.insert("web".into(), ServerOverride { share: Some(Share::Native), http_args: Some(vec!["--port".into(), "{port}".into()]) });
        c.servers.insert("bare".into(), ServerOverride { share: Some(Share::Native), http_args: None });
        let d = decide(&spec("node", &["web.js"]), "web", &c);
        assert_eq!((d.share, d.http_args.unwrap().len()), (Share::Native, 2));
        assert_eq!(decide(&spec("node", &["bare.js"]), "bare", &c).share, Share::Session, "no way to reach it over HTTP");
        c.servers.insert("@playwright/mcp".into(), ServerOverride { share: Some(Share::Native), http_args: None });
        let pw = decide(&spec("npx", &["@playwright/mcp", "--isolated"]), "pw", &c);
        assert_eq!(pw.share, Share::Native, "opted in");
        assert!(pw.http_args.unwrap().contains(&"{port}".to_string()));
        assert_eq!(decide(&spec("npx", &["@playwright/mcp"]), "pw", &c).share, Share::Session, "sharing needs --isolated");
    }
}
