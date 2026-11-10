# Benchmarks

`hangar bench` measures what N agent sessions cost with their MCP servers started the usual way (every session starts its own copy) versus through the hangar pool. `bash bench/run-all.sh` reproduces every table here in about 15 minutes.

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

Machine: Windows 11, AMD Ryzen 5 9600X (6 cores / 12 threads), 31 GB RAM (about 6.5 GB free during this run), Node 22.19, Claude Code 2.1.321.

## Real Claude Code ([claude.md](results/claude.md))

| Sessions | Mode | MCP processes | MCP memory | Claude memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 820 MB | 438 MB | 6.9 s | 3.9 s |
| 1 | pooled | 17 | 346 MB | 430 MB | 2.8 s | 1.4 s |
| 3 | direct | 78 | 2.39 GB | 1.24 GB | 24.3 s | 6.1 s |
| 3 | pooled | 37 | 599 MB | 1.27 GB | 6.9 s | 1.8 s |
| 5 | pooled | 57 | 857 MB | 1.95 GB | 11.8 s | 2.9 s |

The 5-session direct run was skipped by the memory guard: it needed about 6.1 GB plus the 2 GB reserve, and 6.8 GB was free (other programs were using the rest of the machine's 31 GB during this run).

## Simulated agents ([sim.md](results/sim.md))

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 25 | 819 MB | 6.0 s | 2.1 s |
| 1 | pooled | 16 | 345 MB | 2.0 s | 742 ms |
| 5 | direct | 125 | 3.96 GB | 36.7 s | 7.4 s |
| 5 | pooled | 52 | 845 MB | 5.8 s | 1.4 s |
| 10 | pooled | 97 | 1.45 GB | 10.0 s | 2.2 s |

The 10-session direct run was skipped by the memory guard (about 8.0 GB needed plus the reserve, 6.5 GB free). A second wave of sessions joining already-warm servers was ready in 395 ms (1 session), 760 ms p95 (5) and 1.8 s p95 (10). The per-session servers still start cold, so this is not instant.

## Every session uses its browser ([sim-browse.md](results/sim-browse.md))

Each session opens a page titled with its own number and then checks, through `browser_tabs`, that it sees exactly its own page. Every run passed.

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 33 | 1.08 GB | 8.0 s | 2.2 s |
| 1 | pooled | 24 | 629 MB | 4.0 s | 929 ms |
| 3 | direct | 99 | 3.20 GB | 28.8 s | 5.6 s |
| 3 | pooled | 46 | 1.01 GB | 7.7 s | 1.7 s |
| 5 | pooled | 68 | 1.43 GB | 10.6 s | 2.0 s |

## Where the savings come from ([sim-no-fast-npx.md](results/sim-no-fast-npx.md))

Five simulated sessions, with the npx fast path turned off so only sharing is measured:

| | MCP processes | MCP memory | CPU to start | Ready p95 |
|---|---:|---:|---:|---:|
| direct | 125 | 3.96 GB | 36.7 s | 7.4 s |
| pooled, sharing only | 91 | 2.06 GB | 19.6 s | 3.7 s |
| pooled, sharing + fast path | 52 | 845 MB | 5.8 s | 1.4 s |

Context7 and Memory are shared by every session, and Playwright is one process with a context per session. Sequential Thinking and Filesystem keep per-session state, so they still run once per session. The fast path makes those per-session copies cheaper too, which is why it matters even with sharing.

The fast path starts a tagged package (`@latest`, or no version) from npx's cache only if real npx resolved it within the last day. The warmup's direct starts are such runs, and the benchmark records them for its pooled runs, as a machine that used these servers today would have. On a day's first start, expect the "sharing only" row.

## Quality ([verify.md](results/verify.md))

`hangar verify` runs scripted tool calls against each server both ways and compares the answers: 21 of 21 identical, with the pool's request counters proving it served every pooled call. Through the pool a call takes 0.1 ms longer (about 0.9 ms for Playwright over HTTP). Real Claude Code (Haiku) did three MCP-only tasks three times each way: 9 of 9 succeeded both ways, with the same number of turns and the same cost.

## Caveats

- **Leftovers.** Ending sessions by closing stdin, and by killing `claude.exe` ([claude-crash.md](results/claude-crash.md)), left no orphaned processes in either mode on this machine. hangar keeps every server in a kill-on-close job object regardless, but there is no measured win to claim here.
- **The fast path skips npm's registry check** between real npx runs (at most a day apart). `fast_npx = false` checks on every start.
- **One machine.** Numbers depend on the machine, the antivirus, the npm cache and which servers you run. Run `hangar bench` with `--preset <your .mcp.json>` to measure your own setup.
- **The baseline** ([baseline-sim.md](results/baseline-sim.md)) was recorded before any pooling code existed.
- **Run-to-run variation.** Earlier full runs of the same matrix (in git history) gave the same picture within a few percent; for example, 5 simulated sessions measured 846 MB pooled there and 845 MB here.
