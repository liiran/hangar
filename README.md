# hangar

**Run many coding-agent sessions on one machine.** One window for every Claude Code session, one shared copy of each MCP server, and a memory budget that queues new sessions instead of letting your machine thrash.

![hangar: sessions grouped by what they need from you, with an inline approval](docs/img/session-dark.png)

## Results

Three real Claude Code sessions started at once, each with five popular MCP servers (Playwright, Context7, Memory, Sequential Thinking, Filesystem). Measured by `hangar bench --agent claude`, which drives real headless Claude Code and reads readiness from its own `mcp_status` (no prompt is sent, so no tokens are spent).

| 3 sessions | Without hangar | With hangar | Change |
|---|---:|---:|---:|
| MCP memory (private bytes) | 2.39 GB | 599 MB | **−75%** |
| MCP processes | 78 | 37 | −53% |
| CPU spent starting MCP servers | 24.3 s | 6.9 s | **−72%** |
| Time until MCP tools are ready (p95) | 6.1 s | 1.8 s | **3.4× faster** |
| Whole footprint, Claude Code included | 3.64 GB | 1.85 GB | −49% |

Five simulated sessions (the same servers, a minimal MCP client per server instead of Claude): **3.96 GB → 845 MB (−79%)**, startup CPU 36.7 s → 5.8 s (−84%), tools ready in 1.4 s instead of 7.4 s. When every session also drives its browser (3 sessions, each opening a page and checking it sees only its own): **3.20 GB → 1.01 GB (−68%)**. Every run had zero failed sessions and zero leftover processes, in both modes.

Where the savings come from, at 5 simulated sessions:

| | MCP memory | Startup CPU |
|---|---:|---:|
| Every session starts its own servers | 3.96 GB | 36.7 s |
| Shared servers only | 2.06 GB (−48%) | 19.6 s (−47%) |
| Shared servers + npx fast path | 845 MB (−79%) | 5.8 s (−84%) |

The fast-path row assumes npx resolved each tagged package within the last day (the benchmark's warmup does, as daily use would). On the first start of a day, pooled startup looks like the "shared servers only" row.

With hangar, 5 real sessions used 857 MB of MCP memory and 10 simulated ones 1.45 GB. Their unpooled runs were skipped by the benchmark's own memory guard: with about 6.5 GB free, they would have left less than the 2 GB reserve.

Measured on Windows 11, Ryzen 5 9600X (12 threads), 31 GB RAM, Claude Code 2.1.321, Node 22. Full tables, the method, and the caveats are in [bench/README.md](bench/README.md). Rerun everything with `bash bench/run-all.sh`.

## Quality: nothing is traded away

Saving memory would be worthless if sessions got worse answers. `hangar verify` runs the same scripted tool calls against each server started directly and through the pool, then compares the answers (timestamps masked). It also proves, from the pool's own request counters, that the pooled answers really came through the pool and not through the shim's direct fallback.

| | Result |
|---|---|
| Tool answers, direct vs. pooled (Memory, Filesystem, Sequential Thinking, Playwright, Context7) | **21 of 21 identical** |
| Servers whose pooled calls were provably served by the pool | 5 of 5 |
| Extra latency per tool call through the pool (p50, 300 calls) | +0.11 ms (Memory), +0.10 ms (Filesystem), +0.94 ms (Playwright, over HTTP) |
| Real Claude Code (Haiku) tasks that need MCP: store and recall, read a file, read a web page | **9 of 9 succeeded both ways**, same turns, same cost; all 9 pooled runs went through the pool |

The Claude tasks run with Claude's built-in tools turned off, so the only way to succeed is through the MCP server. Full tables: [verify.md](bench/results/verify.md).

## Why

Every Claude Code session starts its own copy of every stdio MCP server, and an `npx`-launched server on Windows is a five-process tree (`cmd` → `node npx-cli` → `cmd` → `node` → `conhost`) of about 150 MB. People running many sessions report what that costs:

- 24 sessions running about 20 copies each of `@playwright/mcp` and another server, roughly 3 GB for the same config, plus kernel OOM kills ([anthropics/claude-code#107433](https://github.com/anthropics/claude-code/issues/107433)).
- About 90 idle sessions exhausting 42 GB of swap on a 32 GB Mac ([#103693](https://github.com/anthropics/claude-code/issues/103693)).
- MCP servers timing out when several sessions start at once ([#107472](https://github.com/anthropics/claude-code/issues/107472)).

And switching between a dozen terminal tabs to find the one that is waiting for you is its own tax.

## What it does

**One window for every session.** Start Claude Code sessions in any folder. The list groups them by what they need from you: *Needs you*, *Working*, *Queued*, *Ready*, *Paused*, and the sidebar filters to what needs you or to one project. Each session reads as an activity feed, with permission requests inline showing the exact command, file contents or diff, and a properties panel for its status, permissions, model, memory, cost and MCP servers. You get a desktop notification when a session needs you, finishes or fails. The layout adapts down to a phone-sized window, and it follows your system's light or dark theme.

Everything is a keystroke away:

| Key | |
|---|---|
| `Ctrl K` | command palette: jump to any session or action |
| `C` | new session |
| `J` / `K` | next / previous session |
| `R` | reply to the open session |
| `Esc` | stop the running turn |
| `Ctrl \` | show or hide the sidebar |

**Shared MCP servers.** `hangar-shim` stands in for each stdio server. All sessions talk to one daemon that runs a single copy of each shareable server. It starts servers on first use, stops idle ones, restarts crashed ones, and keeps every process in a Windows job object so nothing outlives it.

**A memory budget.** A session starts only if, after its expected cost (learned from your real sessions), a reserve of free memory remains. Otherwise it waits in a queue. Before making anyone wait, hangar pauses the longest-idle session (an idle Claude Code process holds 400–470 MB on this machine) and resumes it on its next message with `--resume`, so the conversation continues where it left off.

![The MCP pool: two servers shared by six sessions, about 500 MB saved](docs/img/pool-dark.png)

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

The shim is a 900 KB windowless binary using about 1.2 MB of memory per instance. It forwards newline-delimited JSON-RPC, answers requests a vanished daemon left open, replays the client's `initialize` when it reconnects, and runs the server itself if the daemon is unreachable, so wrapping a server never breaks it.

The daemon gives each server a **share level** and never changes what a server returns:

| Level | Who shares one process | Used for |
|---|---|---|
| `global` | every session | stateless API clients and docs (Context7, GitHub, search, fetch, Memory) |
| `project` | sessions in the same folder | servers you mark `project` in the config |
| `native` | every session, each with its own state inside the server | Playwright with `--isolated`: one browser, an isolated context per session |
| `session` | one session, as today, but supervised | stateful servers (filesystem roots, sequential thinking, browsers without `--isolated`) and every server hangar does not know (the safe default) |

Sessions whose environments carry different credentials (anything that looks like a key, token or secret) never share a process, whatever the level.

For shared servers the daemon:

- rewrites each session's request ids, progress tokens and cancellations, so sessions can reuse ids without seeing each other's traffic;
- reference-counts resource subscriptions;
- caches `tools/list` and similar until the server announces a change;
- initializes the server once, without per-client capabilities (roots, sampling, elicitation), so a shared server can never ask one session something meant for another;
- restarts a crashed server with backoff and tells every session its tool list changed.

Two launch optimizations apply at every level:

- **npx fast path.** npx names its cache folder `sha512(package spec)[..16]`, so hangar can start a cached package's entry script with `node` directly. That skips two launcher processes (about 70 MB) and npm's registry round-trip. Exact versions always qualify; tags like `@latest` only if hangar saw real npx resolve them within the last day, so you still get updates. The entry script must live inside its package folder. If the fast start fails, hangar uses the configured command.
- **No console for the shim.** Windows starts no `conhost.exe` for it.

## How sessions work

Each session is a headless `claude` process speaking the same stream-json control protocol the official Agent SDKs use. Permission prompts arrive as `can_use_tool` requests and wait for you in the UI. Stopped or paused sessions resume the same conversation with `--resume`. Events are kept in `~/.hangar/sessions/<id>/events.jsonl`, so history survives restarts.

A folder's own Claude settings can run commands (hooks), pre-approve tools and start MCP servers. Interactive Claude Code asks before trusting a new folder; headless mode does not, so hangar does: the first session in a folder shows exactly what it would run and starts only after you confirm. "Allow for session" remembers a rule for that session only, never in the project's settings files.

## Security

Everything listens on `127.0.0.1` only, and nothing runs without a secret only your user can read.

- **Daemon and shims.** The daemon writes a random token to `~/.hangar/daemon.json` (owner-only on Unix). A shim first makes the daemon prove it knows the token (a SHA-256 challenge), and only then sends its token, command and environment. A process that does not know the token cannot start anything through the daemon, and a fake daemon cannot collect a shim's environment.
- **The app.** `hangar up` opens the app with a one-time pairing code (128 random bits, single use, valid 2 minutes), never the token. The page trades it for a separate UI token kept in the page's storage and sent as a header, so other sites cannot ride on it the way they could with a cookie. The API also checks `Host` (against DNS rebinding) and `Origin`, and serves a strict Content-Security-Policy. Images in Claude's replies render as links, so a reply cannot make the app fetch a URL.
- **Processes.** Every server and every Claude process runs in a kill-on-close job object, so nothing outlives hangar, even if it crashes. Arguments passed to `claude` are whitelisted (permission modes) or validated (model names cannot start with `-`). Session ids are validated before they touch the disk; config edits are atomic, backed up and never pass through a world-readable file.
- **One gap to know about.** Playwright's multi-session mode is an HTTP server on a random loopback port without authentication of its own, so on a machine shared with other users, another local user could reach it. Set `share = "session"` for Playwright there.

## Commands

| | |
|---|---|
| `hangar up` | start the daemon if needed and open the app |
| `hangar status` | the daemon and every pooled server, with memory and sessions |
| `hangar wrap` / `unwrap` | route Claude Code's stdio MCP servers through the pool, or undo it |
| `hangar bench` | measure memory, CPU and startup time, pooled vs. not, on your own machine |
| `hangar verify` | check pooled servers answer exactly like direct ones and time each call; `--with-claude` also runs real Claude tasks both ways |
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
cargo test --workspace                      # Rust: unit, pool, sessions, API and CLI tests
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
cd web && npm test                          # UI logic
cargo build --workspace && npm run e2e      # the app in a real browser against a real daemon (fake Claude, no network)
cargo build --release && npm run e2e:claude # one real Claude Code (Haiku) session through the API; costs well under a cent
npm run dev                                 # UI dev server on :5173, proxied to a daemon on :7317
```

The integration tests run real shim processes against an in-process daemon, with a scriptable fake MCP server (stdio and HTTP) and a fake `claude` that speaks the stream-json protocol. They cover routing under concurrent overlapping ids, progress isolation, caching, crash restart, idle shutdown, subscriptions, roots safety, fallback, reconnect after a daemon restart, impostor daemons, approvals and withdrawn approvals, the budget queue, idle reclaim, resume, workspace trust, pairing, and the API's token, Host, Origin and CSP checks.

| Path | |
|---|---|
| `crates/hangar-shim` | the stand-in a client launches instead of a server |
| `crates/hangar-proto` | the shim ↔ daemon handshake and discovery file |
| `crates/hangar/src/pool` | upstream actors (stdio and multi-session HTTP), sharing and routing |
| `crates/hangar/src/sessions` | the Claude Code driver, session state, storage and workspace trust |
| `crates/hangar/src/budget.rs` | the memory budget |
| `crates/hangar/src/api.rs` | the app's HTTP and WebSocket API |
| `crates/hangar/src/bench`, `verify.rs` | the benchmark and the quality check |
| `crates/testkit` | fake MCP servers, fake `claude`, and the integration tests |
| `web/` | the React + TypeScript + Tailwind app, its unit tests and `e2e/` scripts |

## Limitations

- Windows is the tested platform. The Unix code paths pass `cargo clippy` for `x86_64-unknown-linux-gnu` and `x86_64-apple-darwin` but have not been run, and `hangar bench` refuses to run there because it relies on Windows job accounting to count every process.
- Claude Code only for now.
- Claude Code 2.1.3xx first probes servers with `server/discover` from a newer MCP revision. hangar answers like today's servers do (method not found), and Claude falls back to `initialize`.
- Between real npx runs (at most a day apart), the fast path starts the version of an `@latest` package that npx last cached. Set `fast_npx = false` to check the registry on every start.
- Killing Claude Code left no orphaned server processes in this benchmark, pooled or not, so that is not claimed as a win. hangar's job objects guarantee it either way.

## License

MIT
