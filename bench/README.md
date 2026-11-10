# Benchmarks

`hangar bench` measures what N agent sessions cost with their MCP servers started the usual way (every session starts its own copy) versus through the hangar pool. `bash bench/run-all.sh` reproduces every table here in about 15 minutes.

## Method

- **Sessions start together.** All N sessions launch at once, which is the case reported to cause MCP timeouts ([anthropics/claude-code#107472](https://github.com/anthropics/claude-code/issues/107472)).
- **Two kinds of agent.**
  - `sim` is a minimal MCP client per server per session, doing what an agent does at startup: `initialize`, `notifications/initialized`, `tools/list`. It is cheap and exact.
  - `claude` runs real headless Claude Code (`--input-format stream-json`) with the servers in `--mcp-config`. Readiness comes from Claude's own `mcp_status` control request (`pending` → `connected`). No prompt is sent, so no tokens are used.
- **One process per run.** Each measured run happens in a fresh child process that puts itself in a new Windows job object. A pooled run also starts a fresh daemon inside that job, so every pooled number includes a cold start.
- **Memory** is private (committed) bytes summed over every process in the job, measured 3 s after the last session is ready. Shared DLL pages are not double counted. "MCP memory" covers servers, their launchers, shims and the daemon; "Agent memory" is `claude.exe`.
- **CPU** comes from job-object accounting (`JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`), so it includes processes that exited during startup (npx starts several). The measuring process's own CPU is subtracted.
- **Ready** is the time from launching a session until all of its servers answered `tools/list` (sim) or reported `connected` (claude).
- **Leftovers** are processes still alive 5 s after every session ended. A pool daemon and the servers it keeps warm on purpose are excluded.
- **Servers**, launched exactly as Claude Code requires on Windows (`cmd /c npx -y <package>`): `@playwright/mcp@latest --headless --isolated`, `@upstash/context7-mcp@latest`, `@modelcontextprotocol/server-memory`, `@modelcontextprotocol/server-sequential-thinking`, `@modelcontextprotocol/server-filesystem <dir>`. Playwright needs `--isolated` to serve more than one session at all: without it, a second session's browser fails with "Browser is already in use", pooled or not.
- **Warm cache.** A warmup pass starts each server once first, so no run measures package downloads.
- **Memory guard.** A run is skipped if, judging by the previous run of the same mode, it would leave less than 2 GB free.

Machine: Windows 11, AMD Ryzen 5 9600X (6 cores / 12 threads), 31 GB RAM, Node 22.19, Claude Code 2.1.321, Playwright MCP 0.0.85.

## Real Claude Code ([claude.md](results/claude.md))

| Sessions | Mode | MCP processes | MCP memory | Claude memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 816 MB | 431 MB | 6.9 s | 3.9 s |
| 1 | pooled | 18 | 347 MB | 421 MB | 3.6 s | 1.8 s |
| 3 | direct | 78 | 2.38 GB | 1.27 GB | 24.8 s | 5.3 s |
| 3 | pooled | 38 | 604 MB | 1.23 GB | 6.8 s | 2.1 s |
| 5 | direct | 130 | 3.97 GB | 2.07 GB | 43.7 s | 8.0 s |
| 5 | pooled | 58 | 870 MB | 2.14 GB | 13.2 s | 3.0 s |

## Simulated agents ([sim.md](results/sim.md))

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 25 | 813 MB | 5.6 s | 2.2 s |
| 1 | pooled | 17 | 344 MB | 2.4 s | 955 ms |
| 5 | direct | 125 | 3.94 GB | 36.8 s | 6.9 s |
| 5 | pooled | 53 | 846 MB | 5.7 s | 1.3 s |
| 10 | pooled | 98 | 1.46 GB | 11.3 s | 1.7 s |

The 10-session direct run was skipped by the memory guard: it needed about 7.9 GB plus the 2 GB reserve, and 9.4 GB was free. A second wave of sessions joining already-warm servers was ready in 606 ms (1 session) and 1.2 s p95 (5 sessions). The per-session servers still start cold, so this is not instant.

## Every session uses its browser ([sim-browse.md](results/sim-browse.md))

Each session opens a page titled with its own number and then checks, through `browser_tabs`, that it sees exactly its own page. Every run passed.

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 33 | 1.08 GB | 8.1 s | 2.1 s |
| 1 | pooled | 25 | 608 MB | 4.1 s | 1.1 s |
| 3 | direct | 99 | 3.20 GB | 28.9 s | 5.3 s |
| 3 | pooled | 47 | 1.02 GB | 7.1 s | 1.2 s |
| 5 | direct | 165 | 5.13 GB | 50.4 s | 9.3 s |
| 5 | pooled | 69 | 1.44 GB | 10.6 s | 1.9 s |

## Where the savings come from ([sim-no-fast-npx.md](results/sim-no-fast-npx.md))

Five simulated sessions, with the npx fast path turned off so only sharing is measured:

| | MCP processes | MCP memory | CPU to start | Ready p95 |
|---|---:|---:|---:|---:|
| direct | 125 | 3.94 GB | 36.8 s | 6.9 s |
| pooled, sharing only | 92 | 2.06 GB | 19.1 s | 4.1 s |
| pooled, sharing + fast path | 53 | 846 MB | 5.7 s | 1.3 s |

Context7 and Memory are shared by every session, and Playwright is one process with a context per session. Sequential Thinking and Filesystem keep per-session state, so they still run once per session. The fast path makes those per-session copies cheaper too, which is why it matters even with sharing.

## Caveats

- **Leftovers.** Ending sessions by closing stdin, and by killing `claude.exe` ([claude-crash.md](results/claude-crash.md)), left no orphaned processes in either mode on this machine. hangar keeps every server in a kill-on-close job object regardless, but there is no measured win to claim here.
- **The fast path skips npm's registry check.** That check is part of why `npx -y pkg@latest` is slow, and skipping it means running the version npx last cached. `fast_npx = false` restores the check.
- **One machine.** Numbers depend on the machine, the antivirus, the npm cache and which servers you run. Run `hangar bench` with `--preset <your .mcp.json>` to measure your own setup.
- **The baseline** ([baseline-sim.md](results/baseline-sim.md)) was recorded before any pooling code existed.
- **Run-to-run variation.** An earlier full run of the same matrix (in git history) gave the same picture within a few percent; for example, 5 real Claude Code sessions measured −76% MCP memory there and −79% here.
