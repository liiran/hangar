# Benchmarks

`hangar bench` measures what N agent sessions cost with their MCP servers started the usual way (every session starts its own copy) versus through the hangar pool. `bash bench/run-all.sh` reproduces every table here in about 25 minutes.

## Method

- **Sessions start together.** All N sessions launch at once, which is the case reported to cause MCP timeouts ([anthropics/claude-code#107472](https://github.com/anthropics/claude-code/issues/107472)).
- **Two kinds of agent.**
  - `sim` is a minimal MCP client per server per session, doing what an agent does at startup: `initialize`, `notifications/initialized`, `tools/list`. It is cheap and exact.
  - `claude` runs real headless Claude Code (`--input-format stream-json`) with the servers in `--mcp-config`. Readiness comes from Claude's own `mcp_status` control request (`pending` → `connected`). No prompt is sent, so no tokens are used.
- **One process per run.** Each measured run happens in a fresh child process that puts itself in a new Windows job object. A pooled run also starts a fresh daemon inside that job, so every pooled number includes a cold start.
- **Memory** is private (committed) bytes summed over every process in the job, measured 3 s after the last session is ready. Shared DLL pages are not double counted. "MCP memory" covers servers, their launchers, shims and the daemon; "Agent memory" is the `claude` processes the benchmark started, identified by process id.
- **CPU** comes from job-object accounting (`JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`), so it includes processes that exited during startup (npx starts several). The measuring process's own CPU is subtracted.
- **Ready** is the time from launching a session until all of its servers answered `tools/list` (sim) or reported `connected` (claude). Percentiles use the nearest-rank method. A session that fails counts as failed, never as fast, and a comparison with any failed session is not reported.
- **Leftovers** are processes still alive 5 s after every session ended. Only the pool daemon and the shared servers it keeps warm on purpose (until their idle timeout) are excluded; a per-session server that outlives its session counts.
- **Servers**, launched exactly as Claude Code requires on Windows (`cmd /c npx -y <package>`): `@playwright/mcp@latest --headless --isolated`, `@upstash/context7-mcp@latest`, `@modelcontextprotocol/server-memory`, `@modelcontextprotocol/server-sequential-thinking`, `@modelcontextprotocol/server-filesystem <dir>`. Playwright needs `--isolated` to serve more than one session at all: without it, a second session's browser fails with "Browser is already in use", pooled or not.
- **Warm cache.** A warmup pass starts each server once first, inside a job object that is closed afterwards, so no run measures package downloads or inherits stray processes.
- **Memory guard.** A run is skipped if, judging by the previous run of the same mode (or a conservative 170 MB per server per session before any run), it would leave less than 2 GB free. A pooled run is never skipped after the direct run with the same session count fit.
- **Windows only.** Counting every process a run starts, including ones that already exited, needs job-object accounting, so `hangar bench` refuses to run elsewhere rather than report partial numbers.

Machine: Windows 11, AMD Ryzen 5 9600X (6 cores / 12 threads), 31 GB RAM (about 9 GB free during this run), Node 22.19, Claude Code 2.1.321. Measured 2026-11-11.

**Playwright runs once per session by default.** One shared Playwright browser is opt-in (`share = "native"`), because it needs a local port without a password ([SECURITY.md](../SECURITY.md)). The tables below use the defaults unless they say "shared browser", which is `--shared-browser`.

## Playwright and Context7 ([claude-pw-c7.md](results/claude-pw-c7.md))

The two local servers people add most, alone: `--preset bench/presets/playwright-context7.json`.

| Real Claude Code sessions | Mode | MCP processes | MCP memory | Claude memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|---:|
| 1 | direct | 11 | 340 MB | 440 MB | 3.3 s | 2.6 s |
| 1 | pooled | 8 | 149 MB | 431 MB | 2.2 s | 1.3 s |
| 3 | direct | 33 | 1016 MB | 1.25 GB | 11.8 s | 3.4 s |
| 3 | pooled | 18 | 294 MB | 1.28 GB | 5.2 s | 1.5 s |
| 3 | pooled, shared browser | 14 | 162 MB | 1.27 GB | 4.4 s | 1.6 s |
| 5 | direct | 55 | 1.65 GB | 2.13 GB | 21.7 s | 5.2 s |
| 5 | pooled | 28 | 441 MB | 2.07 GB | 8.2 s | 2.2 s |
| 5 | pooled, shared browser | 20 | 170 MB | 2.08 GB | 6.3 s | 1.5 s |

- **By default,** Context7 is one process for everyone, and each session's Playwright is a single `node` process instead of a five-process tree. 5 sessions: **1.65 GB → 441 MB (−74%)**.
- **At 1 session** the saving is the npx fast path alone (no `cmd` and `npx` launcher processes).
- **With the shared browser on** ([claude-pw-c7-shared.md](results/claude-pw-c7-shared.md)), the servers cost about the same for 5 sessions as for 1: 170 MB (−90%).

When every session also opens a page in its browser ([sim-browse-pw-c7.md](results/sim-browse-pw-c7.md)):

| Sessions | Direct | Pooled | Pooled, shared browser ([results](results/sim-browse-pw-c7-shared.md)) |
|---:|---:|---:|---:|
| 1 | 609 MB | 415 MB | 400 MB |
| 3 | 1.78 GB | 1.08 GB (−39%) | 567 MB (−68%) |
| 5 | 2.95 GB | 1.74 GB (−41%) | 713 MB (−76%) |

A browser per session is most of what remains by default. The shared browser replaces them with one browser and a private context per session.

Simulated sessions without browsing ([sim-pw-c7.md](results/sim-pw-c7.md)) reach 10: **3.28 GB → 794 MB (−76%)**, 100 processes → 43, tools ready in 1.3 s instead of 7.2 s.

## Five popular servers, real Claude Code ([claude.md](results/claude.md))

| Sessions | Mode | MCP processes | MCP memory | Claude memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 837 MB | 426 MB | 8.4 s | 4.2 s |
| 1 | pooled | 17 | 355 MB | 450 MB | 3.5 s | 1.5 s |
| 3 | direct | 78 | 2.42 GB | 1.27 GB | 25.6 s | 6.5 s |
| 3 | pooled | 41 | 755 MB | 1.30 GB | 8.2 s | 2.0 s |
| 5 | direct | 130 | 3.84 GB | 2.04 GB | 42.3 s | 10.3 s |
| 5 | pooled | 65 | 1.12 GB | 2.04 GB | 13.5 s | 3.0 s |

Three sessions: MCP memory −70%, startup CPU −68%, and the whole footprint with Claude Code included goes from 3.70 GB to 2.04 GB (−45%). Five sessions: 5.88 GB → 3.16 GB (−46%).

## Simulated agents ([sim.md](results/sim.md))

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 25 | 831 MB | 6.5 s | 2.5 s |
| 1 | pooled | 16 | 352 MB | 2.4 s | 806 ms |
| 5 | direct | 125 | 3.95 GB | 38.5 s | 10.6 s |
| 5 | pooled | 60 | 1.11 GB | 7.4 s | 1.9 s |
| 10 | direct | 250 | 7.38 GB | 79.3 s | 21.5 s |
| 10 | pooled | 115 | 2.08 GB | 14.6 s | 2.9 s |

Ten sessions: 7.38 GB → 2.08 GB (−72%), and tools are ready in 2.9 s instead of 21.5 s.

## Every session uses its browser ([sim-browse.md](results/sim-browse.md))

Each session opens a page and checks that it sees only its own.

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 33 | 1.06 GB | 8.9 s | 2.9 s |
| 1 | pooled | 24 | 622 MB | 4.0 s | 891 ms |
| 3 | direct | 99 | 3.20 GB | 29.1 s | 7.1 s |
| 3 | pooled | 62 | 1.51 GB | 12.1 s | 2.4 s |
| 3 | pooled, shared browser ([results](results/sim-browse-shared.md)) | 46 | 1019 MB | 6.8 s | 1.4 s |
| 5 | direct | 165 | 5.19 GB | 49.4 s | 11.6 s |
| 5 | pooled | 100 | 2.41 GB | 20.4 s | 3.5 s |

## Where the savings come from ([sim-no-fast-npx.md](results/sim-no-fast-npx.md))

Five simulated sessions, five servers:

| | MCP processes | MCP memory | CPU to start | Ready p95 |
|---|---:|---:|---:|---:|
| direct | 125 | 3.95 GB | 38.5 s | 10.6 s |
| pooled, sharing only | 111 | 2.72 GB | 25.0 s | 6.6 s |
| pooled, sharing + fast path | 60 | 1.11 GB | 7.4 s | 1.9 s |

Context7 and Memory are shared by every session. Playwright, Sequential Thinking and Filesystem keep per-session state, so they run once per session. The fast path makes those per-session copies much cheaper, which is why it matters even with sharing.

The fast path starts a tagged package (`@latest`, or no version) from npx's cache only if real npx resolved it within the last day. The warmup's direct starts are such runs, and the benchmark records them for its pooled runs, as a machine that used these servers today would have. On a day's first start, expect the "sharing only" row.

## Quality ([verify.md](results/verify.md))

`hangar verify` runs scripted tool calls against each server both ways and compares the answers: 21 of 21 identical, with the pool's request counters proving it served every pooled call. Through the pool a call takes 0.1 ms longer (about 0.9 ms for Playwright over HTTP). Real Claude Code (Haiku) did three MCP-only tasks three times each way: 9 of 9 succeeded both ways, with the same number of turns and the same cost. `verify` turns the shared Playwright browser on, so that path is checked too.

## Caveats

- **Leftovers.** Ending sessions by closing stdin, and by killing `claude.exe` ([claude-crash.md](results/claude-crash.md)), left no orphaned processes in either mode on this machine. hangar keeps every server in a kill-on-close job object regardless, but there is no measured win to claim here.
- **The fast path skips npm's registry check** between real npx runs (at most a day apart). `fast_npx = false` checks on every start.
- **One machine.** Numbers depend on the machine, the antivirus, the npm cache and which servers you run. Run `hangar bench` with `--preset <your .mcp.json>` to measure your own setup.
- **The baseline** ([baseline-sim.md](results/baseline-sim.md)) was recorded before any pooling code existed.
- **Run-to-run variation.** Memory repeats within a few percent between full runs. Startup times vary more with what else the machine is doing.
