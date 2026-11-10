//! `hangar wrap` / `hangar unwrap`: route Claude Code's stdio MCP servers
//! through `hangar-shim`, and back.
//!
//! Wrapping rewrites each stdio server in `~/.claude.json` (user scope and
//! per-project local scope) from
//!
//! ```json
//! {"command": "cmd", "args": ["/c", "npx", "-y", "@playwright/mcp@latest"]}
//! ```
//! to
//! ```json
//! {"command": "C:/.../hangar-shim.exe", "args": ["--name", "playwright", "--", "cmd", "/c", "npx", "-y", "@playwright/mcp@latest"]}
//! ```
//!
//! The original command is kept verbatim after `--`, so unwrapping is exact.
//! A timestamped backup of the file is written first.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub scope: String,
    pub name: String,
    pub before: String,
    pub after: String,
}

pub fn claude_json_path() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    home.join(".claude.json")
}

pub fn shim_path() -> Result<PathBuf> {
    let me = std::env::current_exe()?;
    let shim = me.with_file_name(if cfg!(windows) { "hangar-shim.exe" } else { "hangar-shim" });
    if !shim.is_file() {
        bail!("hangar-shim not found next to {}", me.display());
    }
    Ok(shim)
}

fn is_shim(command: &str) -> bool {
    let base = command.rsplit(['/', '\\']).next().unwrap_or(command).to_ascii_lowercase();
    base == "hangar-shim" || base == "hangar-shim.exe"
}

fn describe(server: &Value) -> String {
    let mut parts = vec![server["command"].as_str().unwrap_or("").to_string()];
    if let Some(args) = server["args"].as_array() {
        parts.extend(args.iter().filter_map(|a| a.as_str().map(String::from)));
    }
    parts.join(" ")
}

/// Every `mcpServers` object in the config: (scope label, json pointer).
fn server_tables(doc: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if doc.get("mcpServers").is_some_and(Value::is_object) {
        out.push(("user".to_string(), "/mcpServers".to_string()));
    }
    if let Some(projects) = doc.get("projects").and_then(Value::as_object) {
        for (path, p) in projects {
            if p.get("mcpServers").and_then(Value::as_object).is_some_and(|m| !m.is_empty()) {
                let escaped = path.replace('~', "~0").replace('/', "~1");
                out.push((format!("project {path}"), format!("/projects/{escaped}/mcpServers")));
            }
        }
    }
    out
}

fn is_stdio(server: &Value) -> bool {
    server.get("command").is_some() && server.get("type").and_then(Value::as_str).is_none_or(|t| t == "stdio")
}

/// Rewrites `doc` in place. Returns what changed.
pub fn wrap_doc(doc: &mut Value, shim: &Path) -> Vec<Change> {
    let mut changes = Vec::new();
    for (scope, ptr) in server_tables(doc) {
        let Some(table) = doc.pointer_mut(&ptr).and_then(Value::as_object_mut) else { continue };
        for (name, server) in table.iter_mut() {
            if !is_stdio(server) || is_shim(server["command"].as_str().unwrap_or_default()) {
                continue;
            }
            let before = describe(server);
            let mut args = vec![json!("--name"), json!(name)];
            if let Some(keys) = server.get("env").and_then(Value::as_object).filter(|e| !e.is_empty()) {
                args.push(json!("--env-keys"));
                args.push(json!(keys.keys().cloned().collect::<Vec<_>>().join(",")));
            }
            args.push(json!("--"));
            args.push(server["command"].clone());
            if let Some(orig) = server.get("args").and_then(Value::as_array) {
                args.extend(orig.iter().cloned());
            }
            server["command"] = json!(shim.display().to_string());
            server["args"] = Value::Array(args);
            changes.push(Change { scope: scope.clone(), name: name.clone(), before, after: describe(server) });
        }
    }
    changes
}

/// Restores every wrapped server to its original command.
pub fn unwrap_doc(doc: &mut Value) -> Vec<Change> {
    let mut changes = Vec::new();
    for (scope, ptr) in server_tables(doc) {
        let Some(table) = doc.pointer_mut(&ptr).and_then(Value::as_object_mut) else { continue };
        for (name, server) in table.iter_mut() {
            if !is_shim(server["command"].as_str().unwrap_or_default()) {
                continue;
            }
            let args = server["args"].as_array().cloned().unwrap_or_default();
            let Some(sep) = args.iter().position(|a| a == "--") else { continue };
            let Some(command) = args.get(sep + 1).cloned() else { continue };
            let before = describe(server);
            server["command"] = command;
            server["args"] = Value::Array(args[sep + 2..].to_vec());
            changes.push(Change { scope: scope.clone(), name: name.clone(), before, after: describe(server) });
        }
    }
    changes
}

/// Stdio servers that are not routed through hangar yet.
pub fn unwrapped(doc: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (scope, ptr) in server_tables(doc) {
        if let Some(table) = doc.pointer(&ptr).and_then(Value::as_object) {
            for (name, server) in table {
                if is_stdio(server) && !is_shim(server["command"].as_str().unwrap_or_default()) {
                    out.push((scope.clone(), name.clone()));
                }
            }
        }
    }
    out
}

/// Applies `edit` to `~/.claude.json` with a backup. `dry_run` only reports.
pub fn edit_file(path: &Path, home: &Path, dry_run: bool, edit: impl FnOnce(&mut Value) -> Vec<Change>) -> Result<Vec<Change>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: Value = serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))?;
    let changes = edit(&mut doc);
    if dry_run || changes.is_empty() {
        return Ok(changes);
    }
    let backups = home.join("backups");
    std::fs::create_dir_all(&backups)?;
    let stamp = crate::pool::upstream::now_ms();
    std::fs::write(backups.join(format!("claude.json.{stamp}")), &text)?;
    let tmp = path.with_extension("json.hangar-tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Value {
        json!({
            "numStartups": 7,
            "mcpServers": {
                "playwright": {"type": "stdio", "command": "cmd", "args": ["/c", "npx", "-y", "@playwright/mcp@latest"], "env": {}},
                "github": {"type": "http", "url": "https://api.githubcopilot.com/mcp/"},
                "keyed": {"command": "npx", "args": ["-y", "tavily-mcp"], "env": {"TAVILY_API_KEY": "secret"}}
            },
            "projects": {
                "C:/code/app": {"mcpServers": {"db": {"command": "node", "args": ["db.js"]}}, "allowedTools": []},
                "C:/code/empty": {"mcpServers": {}}
            }
        })
    }

    #[test]
    fn wraps_only_stdio_servers_and_unwraps_exactly() {
        let original = doc();
        let mut d = original.clone();
        let changes = wrap_doc(&mut d, Path::new("C:/hangar/hangar-shim.exe"));
        let names: Vec<&str> = changes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(!names.contains(&"github"), "remote servers untouched");
        assert_eq!(d["mcpServers"]["playwright"]["args"][0], "--name");
        assert_eq!(d["mcpServers"]["playwright"]["args"][3], "cmd");
        assert_eq!(d["mcpServers"]["keyed"]["args"][2], "--env-keys");
        assert_eq!(d["mcpServers"]["keyed"]["args"][3], "TAVILY_API_KEY");
        assert_eq!(d["mcpServers"]["keyed"]["env"]["TAVILY_API_KEY"], "secret", "env kept for the shim to read");
        assert_eq!(d["projects"]["C:/code/app"]["mcpServers"]["db"]["args"][3], "node");
        assert!(unwrapped(&d).is_empty());
        assert_eq!(wrap_doc(&mut d.clone(), Path::new("x/hangar-shim")).len(), 0, "wrapping twice changes nothing");
        assert_eq!(unwrap_doc(&mut d).len(), 3);
        assert_eq!(d, original, "unwrap restores the exact original");
    }

    #[test]
    fn edit_file_backs_up_and_honors_dry_run() {
        let dir = std::env::temp_dir().join(format!("hangar-cc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        std::fs::write(&path, serde_json::to_string(&doc()).unwrap()).unwrap();
        let shim = PathBuf::from("C:/h/hangar-shim.exe");
        let dry = edit_file(&path, &dir, true, |d| wrap_doc(d, &shim)).unwrap();
        assert_eq!(dry.len(), 3);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("hangar-shim"), "dry run writes nothing");
        edit_file(&path, &dir, false, |d| wrap_doc(d, &shim)).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("hangar-shim"));
        assert_eq!(std::fs::read_dir(dir.join("backups")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
