//! `hangar verify`: proves that pooling changes nothing a client sees, and
//! measures what it costs per call.
//!
//! 1. **Tool parity.** For each real server, run the same scripted tool calls
//!    once with the server started directly and once through the pool, and
//!    compare the answers after masking timestamps.
//! 2. **Per-call latency.** Time many cheap calls both ways.
//! 3. **Agent tasks** (`--with-claude`, costs a few cents): real Claude Code
//!    (Haiku) does the same MCP-dependent tasks both ways, with its built-in
//!    tools turned off so it has to go through MCP.

use crate::bench::{self, Mode};
use crate::client::McpClient;
use crate::procs::Job;
use anyhow::{Context, Result};
use hangar_proto::ServerSpec;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const T: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
pub struct VerifyOpts {
    pub calls: usize,
    pub with_claude: bool,
    pub repeat: usize,
    pub out: Option<PathBuf>,
    pub claude_bin: Option<PathBuf>,
}

/// How strictly two answers must agree.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Expect {
    /// Same answer once timestamps are masked.
    Identical,
    /// Both succeed (the answer comes from the network and may vary).
    BothSucceed,
}

struct Step {
    tool: &'static str,
    /// Alternative tool names used by other versions of the server.
    aliases: &'static [&'static str],
    args: Value,
    expect: Expect,
}

fn step(tool: &'static str, args: Value) -> Step {
    Step { tool, aliases: &[], args, expect: Expect::Identical }
}

struct Server {
    name: &'static str,
    spec: ServerSpec,
    script: Vec<Step>,
    /// A cheap, read-only call used for the latency measurement.
    probe: Option<(&'static str, Value)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub server: String,
    pub step: String,
    pub outcome: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Latency {
    pub server: String,
    pub call: String,
    pub direct_p50_ms: f64,
    pub direct_p95_ms: f64,
    pub pooled_p50_ms: f64,
    pub pooled_p95_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskResult {
    pub task: String,
    pub mode: Mode,
    pub run: usize,
    pub success: bool,
    pub turns: Option<u64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub parity: Vec<StepResult>,
    pub latency: Vec<Latency>,
    pub tasks: Vec<TaskResult>,
}

fn servers(workdir: &Path) -> Vec<Server> {
    let root = workdir.join("files");
    let root_s = root.display().to_string();
    let hello = root.join("hello.txt").display().to_string();
    let mut memory = bench::npx_spec(&["@modelcontextprotocol/server-memory"]);
    memory.env.insert("MEMORY_FILE_PATH".into(), workdir.join("memory.jsonl").display().to_string());
    let page = "data:text/html,<title>hangar verify</title><h1>Parity</h1><p id=n>4217</p>";
    vec![
        Server {
            name: "memory",
            spec: memory,
            script: vec![
                step("create_entities", json!({"entities": [{"name": "hangar-verify", "entityType": "check", "observations": ["pooling keeps results identical"]}]})),
                step("open_nodes", json!({"names": ["hangar-verify"]})),
                step("search_nodes", json!({"query": "identical"})),
                step("delete_entities", json!({"entityNames": ["hangar-verify"]})),
                step("read_graph", json!({})),
            ],
            probe: Some(("read_graph", json!({}))),
        },
        Server {
            name: "filesystem",
            spec: bench::npx_spec(&["@modelcontextprotocol/server-filesystem", &root_s]),
            script: vec![
                step("list_allowed_directories", json!({})),
                step("list_directory", json!({"path": root_s})),
                Step { tool: "read_text_file", aliases: &["read_file"], args: json!({"path": hello}), expect: Expect::Identical },
                step("search_files", json!({"path": root_s, "pattern": "*.txt"})),
            ],
            probe: Some(("list_allowed_directories", json!({}))),
        },
        Server {
            name: "sequential-thinking",
            spec: bench::npx_spec(&["@modelcontextprotocol/server-sequential-thinking"]),
            script: vec![
                step("sequentialthinking", json!({"thought": "Compare pooled and direct answers.", "thoughtNumber": 1, "totalThoughts": 2, "nextThoughtNeeded": true})),
                step("sequentialthinking", json!({"thought": "They should match.", "thoughtNumber": 2, "totalThoughts": 2, "nextThoughtNeeded": false})),
            ],
            probe: None,
        },
        Server {
            name: "playwright",
            spec: bench::npx_spec(&["@playwright/mcp@latest", "--headless", "--isolated"]),
            script: vec![
                step("browser_navigate", json!({"url": page})),
                step("browser_snapshot", json!({})),
                step("browser_evaluate", json!({"function": "() => document.getElementById('n').textContent"})),
                step("browser_tabs", json!({"action": "list"})),
            ],
            probe: Some(("browser_tabs", json!({"action": "list"}))),
        },
        Server {
            name: "context7",
            spec: bench::npx_spec(&["@upstash/context7-mcp@latest"]),
            script: vec![Step { tool: "resolve-library-id", aliases: &[], args: json!({"libraryName": "react", "query": "react hooks"}), expect: Expect::BothSucceed }],
            probe: None,
        },
    ]
}

/// Replaces values that legitimately differ between two runs (timestamps,
/// generated file names, temp paths) so answers can be compared.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < b.len() {
        // ISO-8601 timestamp: dddd-dd-ddT...
        if i + 10 < b.len() && b[i..i + 4].iter().all(char::is_ascii_digit) && b[i + 4] == '-' && b[i + 7] == '-' && (b[i + 10] == 'T' || b[i + 10] == ' ') {
            let mut j = i + 11;
            while j < b.len() && (b[j].is_ascii_digit() || matches!(b[j], ':' | '.' | '-' | 'Z' | '+')) {
                j += 1;
            }
            out.push_str("<time>");
            i = j;
            continue;
        }
        // Long digit runs: epoch times, ports, pids.
        if b[i].is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j - i >= 9 {
                out.push_str("<n>");
            } else {
                out.extend(&b[i..j]);
            }
            i = j;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn first_difference(a: &str, b: &str) -> String {
    let pos = a.chars().zip(b.chars()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let snip = |s: &str| s.chars().skip(pos.saturating_sub(40)).take(120).collect::<String>();
    format!("direct: …{}…\npooled: …{}…", snip(a), snip(b))
}

/// Runs one server's script; returns (step label, outcome, normalized answer).
async fn run_script(client: &McpClient, server: &Server) -> Result<Vec<(String, Option<Value>, String)>> {
    client.initialize("hangar-verify", T).await?;
    let tools = client.call("tools/list", json!({}), T).await?;
    let names: Vec<String> = tools["tools"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str().map(String::from)).collect()).unwrap_or_default();
    let mut out = vec![("tools/list".to_string(), Some(tools.clone()), normalize(&tools.to_string()))];
    for (i, s) in server.script.iter().enumerate() {
        let tool = std::iter::once(s.tool).chain(s.aliases.iter().copied()).find(|t| names.iter().any(|n| n == t));
        let label = format!("{}. {}", i + 1, s.tool);
        let Some(tool) = tool else {
            out.push((label, None, "tool not offered".into()));
            continue;
        };
        let r = client.request("tools/call", json!({"name": tool, "arguments": s.args}), T).await?;
        let body = r.get("result").or_else(|| r.get("error")).cloned().unwrap_or(Value::Null);
        out.push((label, Some(body.clone()), normalize(&body.to_string())));
    }
    Ok(out)
}

fn is_error(v: &Option<Value>) -> bool {
    match v {
        Some(v) => v.get("isError") == Some(&json!(true)) || v.get("code").is_some(),
        None => true,
    }
}

async fn latency(client: &McpClient, call: &(&str, Value), n: usize) -> Result<(f64, f64)> {
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        client.call("tools/call", json!({"name": call.0, "arguments": call.1}), T).await?;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let pick = |p: f64| times[((p / 100.0) * (times.len() - 1) as f64).round() as usize];
    Ok((pick(50.0), pick(95.0)))
}

fn spawn(server: &Server, workdir: &Path, pool: Option<&bench::Pool>) -> Result<McpClient> {
    let cmd = bench::server_command(server.name, &server.spec, workdir, pool);
    McpClient::spawn(cmd, vec![workdir.display().to_string()])
}

pub async fn run(opts: VerifyOpts) -> Result<Report> {
    // Everything started here dies with this process.
    let job = Job::new_kill_on_close()?;
    job.assign_self().context("joining a job")?;
    std::mem::forget(job);

    let workdir = std::env::temp_dir().join("hangar-verify");
    let _ = std::fs::remove_dir_all(&workdir);
    std::fs::create_dir_all(workdir.join("files"))?;
    std::fs::write(workdir.join("files").join("hello.txt"), "hello from hangar verify\n")?;
    std::fs::write(workdir.join("files").join("notes.md"), "# notes\n")?;
    let servers = servers(&workdir);
    println!("hangar verify: {} servers, {} latency calls each", servers.len(), opts.calls);

    let pool = bench::start_pool(&workdir, true).await?;
    let mut parity = Vec::new();
    let mut lat = Vec::new();
    for server in &servers {
        let _ = std::fs::remove_file(workdir.join("memory.jsonl"));
        let direct = spawn(server, &workdir, None)?;
        let d = run_script(&direct, server).await.with_context(|| format!("{} (direct)", server.name))?;
        let _ = std::fs::remove_file(workdir.join("memory.jsonl"));
        let pooled = spawn(server, &workdir, Some(&pool))?;
        let p = run_script(&pooled, server).await.with_context(|| format!("{} (pooled)", server.name))?;
        for ((label, dv, dn), (_, pv, pn)) in d.into_iter().zip(p) {
            let expect = server.script.iter().enumerate().find(|(i, _)| label.starts_with(&format!("{}.", i + 1))).map(|(_, s)| s.expect).unwrap_or(Expect::Identical);
            let (outcome, detail) = if dn == "tool not offered" || pn == "tool not offered" {
                ("skipped".to_string(), Some("tool not offered by this version".to_string()))
            } else if expect == Expect::BothSucceed {
                match (is_error(&dv), is_error(&pv)) {
                    (false, false) => ("both succeeded".into(), None),
                    _ => ("differs".into(), Some(first_difference(&dn, &pn))),
                }
            } else if dn == pn {
                ("identical".into(), None)
            } else {
                ("differs".into(), Some(first_difference(&dn, &pn)))
            };
            println!("  {:<20} {:<28} {}", server.name, label, outcome);
            parity.push(StepResult { server: server.name.into(), step: label, outcome, detail });
        }
        if let Some(probe) = &server.probe {
            let (d50, d95) = latency(&direct, probe, opts.calls).await?;
            let (p50, p95) = latency(&pooled, probe, opts.calls).await?;
            println!("  {:<20} {:<28} direct p50 {d50:.2} ms, pooled p50 {p50:.2} ms", server.name, probe.0);
            lat.push(Latency { server: server.name.into(), call: probe.0.into(), direct_p50_ms: d50, direct_p95_ms: d95, pooled_p50_ms: p50, pooled_p95_ms: p95 });
        }
        direct.shutdown_like_agent().await;
        pooled.shutdown_like_agent().await;
    }

    let mut tasks = Vec::new();
    if opts.with_claude {
        tasks = agent_tasks(&opts, &workdir, &servers, &pool).await?;
    }
    let _ = crate::daemon::send_control(&pool.home, "stop", json!({})).await;

    let report = Report { parity, latency: lat, tasks };
    let md = render(&report);
    println!("\n{md}");
    if let Some(out) = &opts.out {
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(out.with_extension("json"), serde_json::to_vec_pretty(&report)?)?;
        std::fs::write(out.with_extension("md"), md)?;
    }
    Ok(report)
}

struct Task {
    name: &'static str,
    servers: &'static [&'static str],
    prompt: String,
    expect: &'static str,
}

fn tasks() -> Vec<Task> {
    vec![
        Task {
            name: "memory: store and recall",
            servers: &["memory"],
            prompt: "Use the memory tools to create an entity named Hangar of type project with the observation 'shares MCP servers between sessions'. Then read it back with the memory tools and reply with only that observation text.".into(),
            expect: "shares mcp servers between sessions",
        },
        Task {
            name: "filesystem: read a file",
            servers: &["filesystem"],
            prompt: "Use the filesystem tools to read hello.txt in the allowed directory and reply with only its exact contents.".into(),
            expect: "hello from hangar verify",
        },
        Task {
            name: "playwright: read a page",
            servers: &["playwright"],
            prompt: "Use the browser tools to open data:text/html,<title>Q</title><h1>Value: 7319</h1> and reply with only the number shown in the heading.".into(),
            expect: "7319",
        },
    ]
}

async fn agent_tasks(opts: &VerifyOpts, workdir: &Path, servers: &[Server], pool: &bench::Pool) -> Result<Vec<TaskResult>> {
    let claude = opts.claude_bin.clone().or_else(crate::agent::find_claude).context("claude not found")?;
    let mut out = Vec::new();
    for task in tasks() {
        for mode in [Mode::Direct, Mode::Pooled] {
            for run in 1..=opts.repeat.max(1) {
                let _ = std::fs::remove_file(workdir.join("memory.jsonl"));
                let mut table = serde_json::Map::new();
                for s in servers.iter().filter(|s| task.servers.contains(&s.name)) {
                    table.insert(s.name.into(), bench::mcp_config_entry(s.name, &s.spec, if mode == Mode::Pooled { Some(pool) } else { None }));
                }
                let cfg = workdir.join(format!("task-{mode}.json"));
                std::fs::write(&cfg, serde_json::to_vec(&json!({"mcpServers": table}))?)?;
                let allowed: Vec<String> = task.servers.iter().map(|s| format!("mcp__{s}")).collect();
                let mut cmd = tokio::process::Command::new(&claude);
                cmd.arg("-p")
                    .arg(&task.prompt)
                    .args(["--model", "haiku", "--output-format", "json", "--strict-mcp-config", "--no-session-persistence", "--tools", ""])
                    .arg("--mcp-config")
                    .arg(&cfg)
                    .arg("--allowedTools")
                    .arg(allowed.join(" "))
                    .current_dir(workdir.join("files"))
                    .env_remove("CLAUDECODE")
                    .stdin(std::process::Stdio::null());
                crate::procs::hide_window(&mut cmd);
                let output = tokio::time::timeout(Duration::from_secs(300), cmd.output()).await.context("claude timed out")??;
                let v: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
                let answer = v["result"].as_str().unwrap_or_default().to_string();
                let success = v["is_error"] == json!(false) && answer.to_lowercase().contains(task.expect);
                println!("  {:<28} {:<6} run {run}: {}", task.name, mode, if success { "ok" } else { "FAILED" });
                out.push(TaskResult {
                    task: task.name.into(),
                    mode,
                    run,
                    success,
                    turns: v["num_turns"].as_u64(),
                    cost_usd: v["total_cost_usd"].as_f64(),
                    duration_ms: v["duration_ms"].as_u64(),
                    answer: answer.chars().take(200).collect(),
                });
            }
        }
    }
    Ok(out)
}

fn render(r: &Report) -> String {
    let mut s = String::from("## hangar verify\n\n### Tool parity (direct vs pooled)\n\n| Server | Step | Result |\n|---|---|---|\n");
    for p in &r.parity {
        s.push_str(&format!("| {} | {} | {} |\n", p.server, p.step, p.outcome));
    }
    let same = r.parity.iter().filter(|p| p.outcome == "identical" || p.outcome == "both succeeded").count();
    let compared = r.parity.iter().filter(|p| p.outcome != "skipped").count();
    s.push_str(&format!("\n**{same} of {compared} answers matched.** Timestamps and long numbers are masked before comparing; network answers only need to both succeed.\n"));
    if !r.latency.is_empty() {
        s.push_str("\n### Latency per tool call\n\n| Server | Call | Direct p50 | Pooled p50 | Direct p95 | Pooled p95 |\n|---|---|---:|---:|---:|---:|\n");
        for l in &r.latency {
            s.push_str(&format!("| {} | {} | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms |\n", l.server, l.call, l.direct_p50_ms, l.pooled_p50_ms, l.direct_p95_ms, l.pooled_p95_ms));
        }
    }
    if !r.tasks.is_empty() {
        s.push_str("\n### Agent tasks (Claude Code, Haiku, built-in tools off)\n\n| Task | Mode | Succeeded | Avg turns | Avg cost |\n|---|---|---:|---:|---:|\n");
        let mut names: Vec<&str> = r.tasks.iter().map(|t| t.task.as_str()).collect();
        names.dedup();
        for name in names {
            for mode in [Mode::Direct, Mode::Pooled] {
                let runs: Vec<&TaskResult> = r.tasks.iter().filter(|t| t.task == name && t.mode == mode).collect();
                if runs.is_empty() {
                    continue;
                }
                let ok = runs.iter().filter(|t| t.success).count();
                let turns = runs.iter().filter_map(|t| t.turns).sum::<u64>() as f64 / runs.len() as f64;
                let cost = runs.iter().filter_map(|t| t.cost_usd).sum::<f64>() / runs.len() as f64;
                s.push_str(&format!("| {name} | {mode} | {ok}/{} | {turns:.1} | ${cost:.4} |\n", runs.len()));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_masks_only_volatile_values() {
        assert_eq!(normalize("at 2026-11-09T21:24:25.123Z ok"), "at <time> ok");
        assert_eq!(normalize("page-1762723465123.yml"), "page-<n>.yml");
        assert_eq!(normalize("value 4217, port 51234"), "value 4217, port 51234", "short numbers are kept");
        assert_eq!(normalize("héllo"), "héllo");
    }

    #[test]
    fn first_difference_points_at_the_change() {
        let d = first_difference("abc DIRECT xyz", "abc POOLED xyz");
        assert!(d.contains("DIRECT") && d.contains("POOLED"));
    }
}
