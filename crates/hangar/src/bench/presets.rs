//! Server sets to benchmark.

use anyhow::{Context, Result, bail};
use hangar_proto::ServerSpec;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Servers to start, by name.
pub type ServerSet = Vec<(String, ServerSpec)>;

/// `npx -y <args>` the way Claude Code needs it on this platform
/// (`cmd /c npx ...` on Windows).
pub fn npx_spec(args: &[&str]) -> ServerSpec {
    let mut full: Vec<String> = Vec::new();
    let command = if cfg!(windows) {
        full.extend(["/c".into(), "npx".into()]);
        "cmd".to_string()
    } else {
        "npx".to_string()
    };
    full.push("-y".into());
    full.extend(args.iter().map(|s| s.to_string()));
    ServerSpec { command, args: full, env: BTreeMap::new() }
}

/// Built-in server sets.
///
/// `popular` is five widely used servers that need no API keys. `light` is
/// two of them, for quick runs. A path to a JSON file with an `mcpServers`
/// object (Claude Code's format) is also accepted.
pub fn preset(name: &str, workdir: &Path) -> Result<ServerSet> {
    let fs_root = workdir.display().to_string();
    let set = match name {
        "popular" => vec![
            // --isolated: without it a second session's browser fails with
            // "Browser is already in use", pooled or not.
            ("playwright".into(), npx_spec(&["@playwright/mcp@latest", "--headless", "--isolated"])),
            ("context7".into(), npx_spec(&["@upstash/context7-mcp@latest"])),
            ("memory".into(), npx_spec(&["@modelcontextprotocol/server-memory"])),
            ("sequential-thinking".into(), npx_spec(&["@modelcontextprotocol/server-sequential-thinking"])),
            ("filesystem".into(), npx_spec(&["@modelcontextprotocol/server-filesystem", &fs_root])),
        ],
        "light" => {
            vec![("context7".into(), npx_spec(&["@upstash/context7-mcp@latest"])), ("memory".into(), npx_spec(&["@modelcontextprotocol/server-memory"]))]
        }
        path => from_file(path)?,
    };
    if set.is_empty() {
        bail!("preset {name} has no stdio servers");
    }
    Ok(set)
}

fn from_file(path: &str) -> Result<ServerSet> {
    let text = std::fs::read_to_string(path).with_context(|| format!("unknown preset or unreadable file: {path}"))?;
    let v: Value = serde_json::from_str(&text)?;
    let servers = v.get("mcpServers").and_then(Value::as_object).context("file has no mcpServers object")?;
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    Ok(servers
        .iter()
        .filter_map(|(name, s)| {
            let command = s.get("command")?.as_str()?.to_string();
            let env = s
                .get("env")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect())
                .unwrap_or_default();
            Some((name.clone(), ServerSpec { command, args: strings(s.get("args")), env }))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_files() {
        let dir = std::env::temp_dir();
        assert_eq!(preset("popular", &dir).unwrap().len(), 5);
        assert!(preset("nope-not-a-file", &dir).is_err());
        let f = dir.join(format!("hangar-preset-{}.json", std::process::id()));
        std::fs::write(&f, r#"{"mcpServers":{"a":{"command":"node","args":["a.js"],"env":{"K":"v"}},"remote":{"url":"https://x"}}}"#).unwrap();
        let set = preset(f.to_str().unwrap(), &dir).unwrap();
        assert_eq!(set.len(), 1, "remote servers are skipped");
        assert_eq!(set[0].1.env["K"], "v");
        let _ = std::fs::remove_file(f);
    }
}
