//! Locating agent CLIs.

use std::path::{Path, PathBuf};

/// Env var that points at a specific `claude` executable.
pub const ENV_CLAUDE: &str = "HANGAR_CLAUDE";

/// Finds the native Claude Code executable.
///
/// The npm package installs a `claude.cmd` wrapper on Windows; the real
/// binary lives under `node_modules/@anthropic-ai/claude-code/bin`. Driving
/// the wrapper would add a `cmd.exe` per session, so prefer the binary.
pub fn find_claude() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(ENV_CLAUDE).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if cfg!(windows) {
            let exe = dir.join("claude.exe");
            if exe.is_file() {
                return Some(exe);
            }
            if (dir.join("claude.cmd").is_file() || dir.join("claude").is_file())
                && let Some(native) = npm_native(&dir)
            {
                return Some(native);
            }
        } else {
            let bin = dir.join("claude");
            if bin.is_file() {
                return Some(bin);
            }
        }
    }
    None
}

fn npm_native(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join("node_modules").join("@anthropic-ai").join("claude-code").join("bin").join("claude.exe");
    candidate.is_file().then_some(candidate)
}
