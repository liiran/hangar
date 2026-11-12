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
//! Every original entry is recorded in `~/.hangar/wrapped.json`, so unwrap
//! restores it exactly. A private, timestamped backup of the file is written
//! first, and the edit is re-applied if Claude Code writes the file while
//! hangar is working on it.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub scope: String,
    pub name: String,
    pub before: String,
    pub after: String,
}

/// Original entries by `"<json pointer>/<name>"`.
pub type Originals = Map<String, Value>;

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
    crate::util::redact_command(&parts)
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

/// The original command and args inside a wrapped entry, if it has exactly
/// the shape `wrap` produces: `--name N [--env-keys K] -- command args...`.
fn unwrapped_parts(server: &Value) -> Option<(Value, Vec<Value>)> {
    let args = server.get("args")?.as_array()?;
    let mut i = 0;
    if args.get(i)? != "--name" || !args.get(i + 1)?.is_string() {
        return None;
    }
    i += 2;
    if args.get(i).is_some_and(|a| a == "--env-keys") {
        i += 2;
    }
    if args.get(i)? != "--" {
        return None;
    }
    let command = args.get(i + 1)?.clone();
    Some((command, args[i + 2..].to_vec()))
}

/// Rewrites `doc` in place, recording each original in `originals`.
pub fn wrap_doc(doc: &mut Value, shim: &Path, originals: &mut Originals) -> Vec<Change> {
    let mut changes = Vec::new();
    for (scope, ptr) in server_tables(doc) {
        let Some(table) = doc.pointer_mut(&ptr).and_then(Value::as_object_mut) else { continue };
        for (name, server) in table.iter_mut() {
            if !is_stdio(server) || is_shim(server["command"].as_str().unwrap_or_default()) {
                continue;
            }
            let before = describe(server);
            originals.insert(format!("{ptr}/{name}"), server.clone());
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

/// Restores every wrapped server: exactly from `originals` when recorded,
/// otherwise from the command line kept inside the wrapped entry.
pub fn unwrap_doc(doc: &mut Value, originals: &mut Originals) -> Vec<Change> {
    let mut changes = Vec::new();
    for (scope, ptr) in server_tables(doc) {
        let Some(table) = doc.pointer_mut(&ptr).and_then(Value::as_object_mut) else { continue };
        for (name, server) in table.iter_mut() {
            if !is_shim(server["command"].as_str().unwrap_or_default()) {
                continue;
            }
            let before = describe(server);
            let key = format!("{ptr}/{name}");
            let restored = match (originals.remove(&key), unwrapped_parts(server)) {
                // Use the recorded original unless the entry was edited since.
                // The recorded original says whether the entry had `args` at
                // all. Everything else stays as it is now, so a key changed
                // in `env` after wrapping is not rolled back.
                (Some(orig), Some((cmd, args)))
                    if orig.get("command") == Some(&cmd) && orig.get("args").map_or(args.is_empty(), |a| *a == Value::Array(args.clone())) =>
                {
                    let mut s = server.clone();
                    s["command"] = cmd;
                    match (orig.get("args"), s.as_object_mut()) {
                        (Some(a), _) => s["args"] = a.clone(),
                        (None, Some(o)) => {
                            o.remove("args");
                        }
                        (None, None) => {}
                    }
                    s
                }
                (_, Some((cmd, args))) => {
                    let mut s = server.clone();
                    s["command"] = cmd;
                    s["args"] = Value::Array(args);
                    s
                }
                (_, None) => continue, // not in the shape wrap makes: leave it alone
            };
            *server = restored;
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

/// A local server that Claude Code still starts on its own.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Unshared {
    /// `user`, or `project <folder>`.
    pub scope: String,
    pub name: String,
}

/// The local (stdio) servers in the Claude config at `path` that do not go
/// through hangar yet. A missing or unreadable config has none.
pub fn unshared(path: &Path) -> Vec<Unshared> {
    let Some(doc) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
        return Vec::new();
    };
    let clean = crate::util::printable;
    unwrapped(&doc).into_iter().map(|(scope, name)| Unshared { scope: clean(&scope), name: clean(&name) }).collect()
}

/// `hangar wrap` (`on`) and `hangar unwrap`: sends the local servers in the
/// Claude config at `path` through hangar, or puts the original entries back.
pub fn set_shared(path: &Path, home: &Path, on: bool, dry_run: bool) -> Result<Vec<Change>> {
    if on {
        let shim = shim_path()?;
        edit_file(path, home, dry_run, |doc, originals| wrap_doc(doc, &shim, originals))
    } else {
        edit_file(path, home, dry_run, unwrap_doc)
    }
}

fn originals_path(home: &Path) -> PathBuf {
    home.join("wrapped.json")
}

fn load_originals(home: &Path) -> Originals {
    std::fs::read(originals_path(home)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Applies `edit` to the Claude config with a backup. `dry_run` only reports.
///
/// If the file changes while we work (Claude Code writes it often), the
/// edit is re-applied to the new content, up to a few times.
pub fn edit_file(path: &Path, home: &Path, dry_run: bool, edit: impl Fn(&mut Value, &mut Originals) -> Vec<Change>) -> Result<Vec<Change>> {
    // Edit the real file behind a symlink, keeping the link.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for _ in 0..5 {
        let text = std::fs::read_to_string(&target).with_context(|| format!("reading {}", path.display()))?;
        let mut doc: Value = serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON; not touching it", path.display()))?;
        let mut originals = load_originals(home);
        let changes = edit(&mut doc, &mut originals);
        if dry_run || changes.is_empty() {
            return Ok(changes);
        }
        let new_text = serde_json::to_vec_pretty(&doc)?;
        // Did anyone write the file while we were editing it?
        if std::fs::read_to_string(&target).ok().as_deref() != Some(text.as_str()) {
            continue;
        }
        let backups = home.join("backups");
        std::fs::create_dir_all(&backups)?;
        crate::util::make_private(&backups);
        crate::util::write_private(&backups.join(format!("claude.json.{}", crate::util::now_ms())), text.as_bytes())?;
        // The file holds API keys: never let it be readable by others, even
        // for a moment, then give it back its own permissions.
        let tmp = target.with_extension("json.hangar-tmp");
        crate::util::write_private(&tmp, &new_text)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, &target)?;
        crate::util::write_private(&originals_path(home), &serde_json::to_vec_pretty(&originals)?)?;
        return Ok(changes);
    }
    bail!("{} kept changing while hangar edited it; close Claude Code sessions and try again", path.display())
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
                "keyed": {"command": "npx", "args": ["-y", "tavily-mcp"], "env": {"TAVILY_API_KEY": "secret"}},
                "bare": {"command": "my-server"}
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
        let mut originals = Originals::new();
        let changes = wrap_doc(&mut d, Path::new("C:/hangar/hangar-shim.exe"), &mut originals);
        let names: Vec<&str> = changes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(!names.contains(&"github"), "remote servers untouched");
        assert_eq!(d["mcpServers"]["playwright"]["args"][3], "cmd");
        assert_eq!(d["mcpServers"]["keyed"]["args"][2], "--env-keys");
        assert_eq!(d["mcpServers"]["keyed"]["env"]["TAVILY_API_KEY"], "secret", "env kept for the shim to read");
        assert!(!changes.iter().any(|c| c.before.contains("secret") || c.after.contains("secret")));
        assert!(unwrapped(&d).is_empty());
        assert_eq!(wrap_doc(&mut d.clone(), Path::new("x/hangar-shim"), &mut Originals::new()).len(), 0, "wrapping twice changes nothing");
        assert_eq!(unwrap_doc(&mut d, &mut originals).len(), 4);
        assert_eq!(d, original, "unwrap restores the exact original, even entries without args");
        assert!(originals.is_empty());
    }

    #[test]
    fn unwrap_without_records_uses_the_wrapped_command_line() {
        let mut d = doc();
        wrap_doc(&mut d, Path::new("C:/h/hangar-shim.exe"), &mut Originals::new());
        unwrap_doc(&mut d, &mut Originals::new());
        assert_eq!(d["mcpServers"]["playwright"]["command"], "cmd");
        assert_eq!(d["mcpServers"]["playwright"]["args"], json!(["/c", "npx", "-y", "@playwright/mcp@latest"]));
    }

    #[test]
    fn unwrap_leaves_unfamiliar_shim_entries_alone() {
        let mut d = json!({"mcpServers": {"x": {"command": "hangar-shim", "args": ["--", "tool"]}}});
        assert!(unwrap_doc(&mut d, &mut Originals::new()).is_empty());
        assert_eq!(unwrapped_parts(&json!({"args": ["--name", "--", "--", "cmd"]})), Some((json!("cmd"), vec![])), "a server literally named --");
    }

    #[test]
    fn edit_file_backs_up_and_honors_dry_run() {
        let dir = std::env::temp_dir().join(format!("hangar-cc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        std::fs::write(&path, serde_json::to_string(&doc()).unwrap()).unwrap();
        let shim = PathBuf::from("C:/h/hangar-shim.exe");
        let dry = edit_file(&path, &dir, true, |d, o| wrap_doc(d, &shim, o)).unwrap();
        assert_eq!(dry.len(), 4);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("hangar-shim"), "dry run writes nothing");
        edit_file(&path, &dir, false, |d, o| wrap_doc(d, &shim, o)).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("hangar-shim"));
        assert_eq!(std::fs::read_dir(dir.join("backups")).unwrap().count(), 1);
        edit_file(&path, &dir, false, unwrap_doc).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(), doc());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
