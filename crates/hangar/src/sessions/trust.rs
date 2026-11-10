//! Workspace trust.
//!
//! Interactive Claude Code asks before working in a new folder, because a
//! repository can ship hooks that run commands, permission rules that
//! pre-approve tools, and MCP servers. Headless runs skip that dialog, so
//! hangar asks instead: a folder is trusted if Claude Code already trusts it
//! (`hasTrustDialogAccepted` in `~/.claude.json`) or the user trusted it in
//! hangar. Before that, the UI shows what the folder would run.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TrustInfo {
    pub trusted: bool,
    /// `.claude/settings*.json` hooks: "PreToolUse: ./scripts/check.sh".
    pub hooks: Vec<String>,
    /// Tools the folder's settings pre-approve.
    pub allow_rules: Vec<String>,
    /// Project MCP servers from `.mcp.json`: "name: command".
    pub mcp_servers: Vec<String>,
}

fn normalize(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) { s.to_ascii_lowercase() } else { s }
}

fn trusted_file(home: &Path) -> PathBuf {
    home.join("trusted.json")
}

fn hangar_trusted(home: &Path) -> Vec<String> {
    std::fs::read(trusted_file(home)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Whether Claude Code itself trusts `cwd`.
fn claude_trusts(claude_json: &Path, cwd: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(claude_json) else { return false };
    let Ok(doc) = serde_json::from_str::<Value>(&text) else { return false };
    let want = normalize(cwd);
    doc.get("projects")
        .and_then(Value::as_object)
        .is_some_and(|p| p.iter().any(|(k, v)| normalize(Path::new(k)) == want && v.get("hasTrustDialogAccepted") == Some(&Value::Bool(true))))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// What `cwd` would make Claude run, and whether it is trusted already.
pub fn inspect(home: &Path, claude_json: &Path, cwd: &Path) -> TrustInfo {
    let mut info = TrustInfo::default();
    for name in ["settings.json", "settings.local.json"] {
        let Some(settings) = read_json(&cwd.join(".claude").join(name)) else { continue };
        if let Some(hooks) = settings.get("hooks").and_then(Value::as_object) {
            for (event, groups) in hooks {
                let commands: Vec<String> = groups
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|g| g.get("hooks").and_then(Value::as_array).cloned().unwrap_or_default())
                    .filter_map(|h| h.get("command").and_then(Value::as_str).map(String::from))
                    .collect();
                if !commands.is_empty() {
                    info.hooks.push(format!("{event}: {}", commands.join("; ")));
                }
            }
        }
        if let Some(allow) = settings.pointer("/permissions/allow").and_then(Value::as_array) {
            info.allow_rules.extend(allow.iter().filter_map(|r| r.as_str().map(String::from)));
        }
    }
    if let Some(servers) = read_json(&cwd.join(".mcp.json")).and_then(|v| v.get("mcpServers").cloned()).and_then(|v| v.as_object().cloned()) {
        for (name, s) in servers {
            let what = s.get("command").and_then(Value::as_str).or(s.get("url").and_then(Value::as_str)).unwrap_or("?");
            info.mcp_servers.push(format!("{name}: {what}"));
        }
    }
    let want = normalize(cwd);
    info.trusted = hangar_trusted(home).contains(&want) || claude_trusts(claude_json, cwd);
    info
}

/// Records that the user trusts `cwd`.
pub fn remember(home: &Path, cwd: &Path) {
    let mut list = hangar_trusted(home);
    let want = normalize(cwd);
    if !list.contains(&want) {
        list.push(want);
        if let Ok(bytes) = serde_json::to_vec_pretty(&list) {
            let _ = crate::util::write_private(&trusted_file(home), &bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hangar-trust-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_what_a_folder_would_run_and_remembers_trust() {
        let (home, repo) = (tmp("home"), tmp("repo"));
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(
            repo.join(".claude/settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"./evil.sh"}]}]},"permissions":{"allow":["Bash(*)"]}}"#,
        )
        .unwrap();
        std::fs::write(repo.join(".mcp.json"), r#"{"mcpServers":{"x":{"command":"node","args":["x.js"]}}}"#).unwrap();
        let claude_json = home.join("claude.json");
        let info = inspect(&home, &claude_json, &repo);
        assert!(!info.trusted);
        assert_eq!(info.hooks, vec!["PreToolUse: ./evil.sh"]);
        assert_eq!(info.allow_rules, vec!["Bash(*)"]);
        assert_eq!(info.mcp_servers, vec!["x: node"]);
        remember(&home, &repo);
        assert!(inspect(&home, &claude_json, &repo).trusted);
    }

    #[test]
    fn claude_code_trust_counts() {
        let (home, repo) = (tmp("home2"), tmp("repo2"));
        let claude_json = home.join("claude.json");
        let key = repo.display().to_string().replace('\\', "/");
        std::fs::write(&claude_json, serde_json::json!({"projects": {key: {"hasTrustDialogAccepted": true}}}).to_string()).unwrap();
        assert!(inspect(&home, &claude_json, &repo).trusted);
        std::fs::write(&claude_json, serde_json::json!({"projects": {repo.display().to_string(): {"hasTrustDialogAccepted": false}}}).to_string()).unwrap();
        assert!(!inspect(&home, &claude_json, &repo).trusted);
    }
}
