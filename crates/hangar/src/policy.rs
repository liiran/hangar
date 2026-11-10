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
//! `project`, so they are only shared between sessions that started them with
//! the same command, environment and folder. Anything can be overridden in
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
    let overridden = config
        .servers
        .get(name)
        .or_else(|| package.as_ref().and_then(|p| config.servers.get(p)))
        .and_then(|o| o.share);
    if let Some(share) = overridden {
        return Decision { share, package, reason: "set in config.toml".into() };
    }
    if let Some(pkg) = &package {
        if pkg == "@playwright/mcp" {
            return if crate::pool::http::native_args(Some(pkg), spec).is_some() {
                Decision { share: Share::Native, package, reason: "one browser, a separate isolated context per session".into() }
            } else {
                Decision {
                    share: Share::Session,
                    package,
                    reason: "without --isolated, Playwright keeps one persistent profile that only one session can use".into(),
                }
            };
        }
        if let Some((_, share, reason)) = REGISTRY.iter().find(|(p, _, _)| p == pkg) {
            return Decision { share: *share, package, reason: (*reason).into() };
        }
    }
    Decision { share: Share::Project, package, reason: "unknown server: shared only by sessions in the same folder with the same config".into() }
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
        assert_eq!(decide(&spec("npx", &["-y", "@playwright/mcp", "--isolated"]), "pw", &c).share, Share::Native);
        assert_eq!(decide(&spec("node", &["mine.js"]), "mine", &c).share, Share::Project);
    }

    #[test]
    fn config_overrides_by_name_or_package() {
        let mut c = Config::default();
        c.servers.insert("pw".into(), ServerOverride { share: Some(Share::Off) });
        c.servers.insert("@upstash/context7-mcp".into(), ServerOverride { share: Some(Share::Session) });
        assert_eq!(decide(&spec("npx", &["@playwright/mcp"]), "pw", &c).share, Share::Off);
        assert_eq!(decide(&spec("npx", &["@upstash/context7-mcp"]), "docs", &c).share, Share::Session);
    }
}
