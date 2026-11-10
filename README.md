# hangar

**Run many coding-agent sessions on one machine.** One window for every Claude Code session, one shared copy of each MCP server, and a memory budget that queues new sessions instead of letting your machine thrash.

![hangar: sessions grouped by what they need from you, with an inline approval](docs/img/session-dark.png)

## Results

Five real Claude Code sessions started at once, each with five popular MCP servers (Playwright, Context7, Memory, Sequential Thinking, Filesystem). Measured by `hangar bench --agent claude`, which drives real headless Claude Code and reads readiness from its own `mcp_status` (no prompt is sent, so no tokens are spent).

| 5 sessions | Without hangar | With hangar | Change |
|---|---:|---:|---:|
| MCP memory (private bytes) | 3.97 GB | 870 MB | **−79%** |
| MCP processes | 130 | 58 | −55% |
| CPU spent starting MCP servers | 43.7 s | 13.2 s | **−70%** |
| Time until MCP tools are ready (p95) | 8.0 s | 3.0 s | **2.7× faster** |
| Whole footprint, Claude Code included | 6.04 GB | 2.99 GB | −50% |

When every session also uses its browser (5 simulated sessions, each opening a page and checking that it sees only its own): **5.13 GB → 1.44 GB (−72%)**, tools ready in 1.9 s instead of 9.3 s.

Where the savings come from, at 5 simulated sessions:

| | MCP memory | Startup CPU |
|---|---:|---:|
| Every session starts its own servers | 3.94 GB | 36.8 s |
| Shared servers only | 2.06 GB (−48%) | 19.1 s (−48%) |
| Shared servers + npx fast path | 846 MB (−79%) | 5.7 s (−84%) |

At 10 simulated sessions the pooled setup used 1.46 GB. The unpooled run was skipped by the benchmark's own memory guard: it needed about 7.9 GB plus a 2 GB reserve, and 9.4 GB was free.

Measured on Windows 11, Ryzen 5 9600X (12 threads), 31 GB RAM, Claude Code 2.1.321, Node 22. Full tables, the method, and the caveats are in [bench/README.md](bench/README.md). Rerun everything with `bash bench/run-all.sh`.

## Why

Every Claude Code session starts its own copy of every stdio MCP server, and an `npx`-launched server on Windows is a five-process tree (`cmd` → `node npx-cli` → `cmd` → `node` → `conhost`) of about 150 MB. People running many sessions report what that costs:

- 24 sessions running about 20 copies each of `@playwright/mcp` and another server, roughly 3 GB for the same config, plus kernel OOM kills ([anthropics/claude-code#107433](https://github.com/anthropics/claude-code/issues/107433)).
- About 90 idle sessions exhausting 42 GB of swap on a 32 GB Mac ([#103693](https://github.com/anthropics/claude-code/issues/103693)).
- MCP servers timing out when several sessions start at once ([#107472](https://github.com/anthropics/claude-code/issues/107472)).

And switching between a dozen terminal tabs to find the one that is waiting for you is its own tax.

## What it does

**One window for every session.** Start Claude Code sessions in any folder. The sidebar groups them by what they need from you: *Needs you*, *Working*, *Queued*, *Ready for you*, *Paused*. Permission requests show up inline with the exact command or file contents, and you get a desktop notification when a session needs you or finishes.

**Shared MCP servers.** `hangar-shim` stands in for each stdio server. All sessions talk to one daemon that runs a single copy of each shareable server. It starts servers on first use, stops idle ones, restarts crashed ones, and keeps every process in a Windows job object so nothing outlives it.

**A memory budget.** A session starts only if, after its expected cost (learned from your real sessions), a reserve of free memory remains. Otherwise it waits in a queue. Before making anyone wait, hangar pauses the longest-idle session (an idle Claude Code process holds 400–470 MB on this machine) and resumes it on its next message with `--resume`, so the conversation continues where it left off.

![The MCP pool panel: two servers shared by six sessions](docs/img/pool-dark.png)

## Quick start

Requirements: Rust 1.89+, Node 22+, Claude Code (`npm i -g @anthropic-ai/claude-code`), Windows 10+. Linux and macOS pass `cargo check` but have not been run yet.

```sh
cd web && npm ci && npm run build && cd ..
cargo build --release
./target/release/hangar up          # starts the daemon and opens the app
./target/release/hangar wrap        # optional: pool the MCP servers of every Claude Code session
```

`hangar wrap` rewrites the stdio servers in `~/.claude.json` to start through `hangar-shim` (backup first, `--dry-run` to preview, `hangar unwrap` to undo exactly). Sessions you start in a terminal then share servers too.

On Windows with the GNU toolchain, put a MinGW `bin` folder (for `dlltool` and `as`) on `PATH`. The MSVC toolchain needs nothing extra.

## How the pool works

```
 Claude Code session A ──stdio──> hangar-shim ─┐
 Claude Code session B ──stdio──> hangar-shim ─┼─loopback──> hangar daemon ──stdio──> context7   (one process)
 Claude Code session C ──stdio──> hangar-shim ─┘   (token)        │      └──stdio──> memory     (one process)
                                                                  └─HTTP, one MCP session each──> playwright (one browser,
                                                                                                    a context per session)
```

The shim is an 800 KB windowless binary using about 1.2 MB of memory per instance. It forwards newline-delimited JSON-RPC, replays the client's `initialize` if the daemon restarts, and runs the server itself if the daemon is unreachable, so wrapping a server never breaks it.

The daemon gives each server a **share level** and never changes what a server returns:

| Level | Who shares one process | Used for |
|---|---|---|
| `global` | every session | stateless API clients and docs (Context7, GitHub, search, fetch, Memory) |
| `project` | sessions in the same folder | unknown servers (the safe default) |
| `native` | every session, each with its own state inside the server | Playwright with `--isolated`: one browser, an isolated context per session |
| `session` | one session, as today, but supervised | stateful servers (filesystem roots, sequential thinking, browsers without `--isolated`) |

For shared servers the daemon:

- rewrites each session's request ids, progress tokens and cancellations, so sessions can reuse ids without seeing each other's traffic;
- reference-counts resource subscriptions;
- caches `tools/list` and similar until the server announces a change;
- initializes the server once, without per-client capabilities (roots, sampling, elicitation), so a shared server can never ask one session something meant for another;
- restarts a crashed server with backoff and tells every session its tool list changed.

Two launch optimizations apply at every level:

- **npx fast path.** npx names its cache folder `sha512(package spec)[..16]`, so hangar can start a cached package's entry script with `node` directly. That skips two launcher processes (about 70 MB) and npm's registry round-trip. If the fast start fails, hangar uses the configured command.
- **No console for the shim.** Windows starts no `conhost.exe` for it.

## How sessions work

Each session is a headless `claude` process speaking the same stream-json control protocol the official Agent SDKs use. Permission prompts arrive as `can_use_tool` requests and wait for you in the UI. Stopped or paused sessions resume the same conversation with `--resume`. Events are kept in `~/.hangar/sessions/<id>/events.jsonl`, so history survives restarts.

The app is served on `127.0.0.1` only. It needs an HttpOnly, SameSite=Strict cookie that `hangar up` sets from a token only your user can read, checks the `Host` header against DNS rebinding, and refuses cross-origin writes.

## Commands

| | |
|---|---|
| `hangar up` | start the daemon if needed and open the app |
| `hangar status` | the daemon and every pooled server, with memory and sessions |
| `hangar wrap` / `unwrap` | route Claude Code's stdio MCP servers through the pool, or undo it |
| `hangar bench` | measure memory, CPU and startup time, pooled vs. not, on your own machine |
| `hangar stop` | stop the daemon and every server it runs |

## Configuration

`~/.hangar/config.toml` (all optional):

```toml
[pool]
idle_timeout_secs = 600     # stop a shared server after 10 idle minutes
fast_npx = true             # start cached npx packages with node directly

[servers.my-server]         # by config name or package name
share = "global"            # global | project | native | session | off

[budget]
reserve_mb = 4096           # default: max(2 GB, 10% of RAM)
max_sessions = 0            # 0 = no fixed cap
reclaim_idle_after_secs = 30
hibernate_after_mins = 0    # pause idle sessions on a timer; 0 = only when room is needed
```

## Development

```sh
cargo test --release --workspace   # 67 tests: unit, pool integration, sessions and API
cd web && npm run dev               # UI dev server, proxied to a running daemon on :7317
```

The integration tests run real shim processes against an in-process daemon, with a scriptable fake MCP server and a fake `claude` that speaks the stream-json protocol. They cover routing under concurrent overlapping ids, progress isolation, caching, crash restart, idle shutdown, subscriptions, roots safety, fallback, reconnect after a daemon restart, approvals, the budget queue, idle reclaim, resume, and the API's cookie, Host and Origin checks.

| Path | |
|---|---|
| `crates/hangar-shim` | the stand-in a client launches instead of a server |
| `crates/hangar/src/pool` | upstream actors (stdio and multi-session HTTP), sharing and routing |
| `crates/hangar/src/sessions` | the Claude Code driver, session state and the budget |
| `crates/hangar/src/bench.rs` | the benchmark harness |
| `web/` | the React + TypeScript + Tailwind app |

## Limitations

- Windows is the tested platform. The Unix code paths type-check (`cargo check --target x86_64-unknown-linux-gnu` and `x86_64-apple-darwin`) but have not been run.
- Claude Code only for now; Codex is next.
- Claude Code 2.1.3xx first probes servers with `server/discover` from a newer MCP revision. hangar answers like today's servers do (method not found), and Claude falls back to `initialize`.
- The npx fast path skips npm's update check for `@latest` packages, so it runs the version npx last cached. Set `fast_npx = false` to keep the check.
- Killing Claude Code left no orphaned server processes in this benchmark, pooled or not, so that is not claimed as a win. hangar's job objects guarantee it either way.

## License

MIT
