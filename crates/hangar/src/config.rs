//! `~/.hangar/config.toml`.
//!
//! ```toml
//! [pool]
//! idle_timeout_secs = 600      # stop a shared server after 10 idle minutes
//!
//! [servers.playwright]         # by config name or npm/PyPI package
//! share = "session"            # global | project | session | off
//!
//! [budget]
//! reserve_mb = 3072            # keep this much RAM free before starting sessions
//! max_sessions = 0             # 0 = no fixed cap
//! ```

use crate::policy::Share;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub pool: PoolConfig,
    pub servers: BTreeMap<String, ServerOverride>,
    pub budget: BudgetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolConfig {
    /// Stop a shared server after it has had no sessions for this long.
    pub idle_timeout_secs: u64,
    /// Stop a per-session server this long after its session leaves.
    pub session_grace_secs: u64,
    /// Give up on a server that has not answered `initialize` by then.
    pub init_timeout_secs: u64,
    /// Start cached `npx` packages with `node` directly instead of through
    /// npx. Skips npm's registry check, saving about a second and ~70 MB.
    pub fast_npx: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig { idle_timeout_secs: 600, session_grace_secs: 2, init_timeout_secs: 90, fast_npx: true }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerOverride {
    pub share: Option<Share>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Keep at least this much physical memory available. 0 = automatic
    /// (the larger of 2 GB and 10% of RAM).
    pub reserve_mb: u64,
    /// Never run more than this many sessions at once. 0 = no fixed cap.
    pub max_sessions: usize,
    /// Assumed cost of a new session until real ones have been measured.
    pub initial_estimate_mb: u64,
    /// Stop the agent process of a session that has been idle this long and
    /// resume it on the next message. 0 = never.
    pub hibernate_after_mins: u64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig { reserve_mb: 0, max_sessions: 0, initial_estimate_mb: 600, hibernate_after_mins: 0 }
    }
}

impl PoolConfig {
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_secs)
    }
    pub fn session_grace(&self) -> Duration {
        Duration::from_secs(self.session_grace_secs)
    }
    pub fn init_timeout(&self) -> Duration {
        Duration::from_secs(self.init_timeout_secs.max(1))
    }
}

impl Config {
    pub fn load(home: &Path) -> Result<Config> {
        let path = home.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_config() {
        let c: Config = toml::from_str("[pool]\nidle_timeout_secs = 5\n[servers.playwright]\nshare = \"off\"\n").unwrap();
        assert_eq!(c.pool.idle_timeout_secs, 5);
        assert_eq!(c.pool.init_timeout_secs, 90);
        assert_eq!(c.servers["playwright"].share, Some(Share::Off));
        assert_eq!(c.budget.initial_estimate_mb, 600);
    }

    #[test]
    fn missing_file_means_defaults() {
        let dir = std::env::temp_dir().join(format!("hangar-cfg-{}", std::process::id()));
        let c = Config::load(&dir).unwrap();
        assert_eq!(c.pool.idle_timeout_secs, 600);
    }
}
