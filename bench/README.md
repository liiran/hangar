# Benchmarks

**The question:** what do the MCP servers of N Claude Code sessions cost in memory, processes, CPU and start-up time, with hangar and without it?

`hangar bench` answers it by running the same sessions two ways on the same machine:

- **direct**, the usual way: every session starts its own copy of every server.
- **pooled**, through hangar: servers that can be shared run once, and the others start as one process each.

**Test machine:** Windows 11 · AMD Ryzen 5 9600X (6 cores, 12 threads) · 31 GB RAM · Node 22.19 · Claude Code 2.1.321 · measured 2026-11-11.

`bash bench/run-all.sh` reproduces every table here in about 50 minutes.

## The short version

MCP memory for 5 sessions:

| Servers and what the sessions do | Without hangar | With hangar (default) | With hangar, shared browser on |
|---|---:|---:|---:|
| Playwright + Context7, no page opened yet (real Claude Code) | 1.65 GB | 441 MB (−74%) | 170 MB (−90%) |
| Playwright + Context7, every session has a page open | 2.95 GB | 1.74 GB (−41%) | 713 MB (−76%) |
| Five servers, no page opened yet (real Claude Code) | 3.84 GB | 1.12 GB (−71%) | 896 MB (−77%) |
| Five servers, every session has a page open | 5.19 GB | 2.41 GB (−54%) | 1.39 GB (−73%) |

"Shared browser" is a setting that is off by default. [Playwright: a browser per session, or one for all](#playwright-a-browser-per-session-or-one-for-all) explains it.

## How to read the tables

**Modes**

| Mode | What runs |
|---|---|
| `direct` | No hangar. Every session starts every server itself. |
| `pooled` | hangar with its defaults. Context7 and Memory run once for all sessions. Playwright, Sequential Thinking and Filesystem run once per session, each as a single process. |
| `pooled, shared browser` | The same, plus the opt-in setting that makes one Playwright serve every session. |

**Columns**

| Column | Meaning |
|---|---|
| Sessions | How many sessions were started, all at the same moment. |
| MCP processes | Every process the servers needed. A server started with `npx` is not one process but a small tree: launcher programs (`cmd`, `npx`) and the server itself. With hangar, the shims and the daemon are counted here too. |
| MCP memory | Memory used by all of those processes together. |
| Claude memory | Memory used by the Claude Code processes themselves. hangar does not change it. It is shown so you can see the whole footprint. |
| CPU to start | Processor time spent until every session was ready, added up over all cores. |
| Ready p95 | How long a session waited until all its tools were usable. p95 is the time that 95% of sessions beat. With 5 sessions, that is the slowest one. |

*Example row.* `5 | direct | 55 | 1.65 GB | 2.13 GB | 21.7 s | 5.2 s` means: five sessions without hangar needed 55 server processes holding 1.65 GB, next to 2.13 GB for Claude Code itself. Starting them took 21.7 seconds of processor time, and the slowest session waited 5.2 seconds for its tools.

**Result files.** Each heading links to the raw output of the run behind it, in `bench/results/`: a `.md` file with what `hangar bench` printed, and a `.json` file with every number. Names say what was run. For example, `claude-pw-c7-shared.md` is real **Claude** Code with **P**lay**w**right and **C**ontext**7**, with the **shared** browser on.

## How it is measured

**Sessions start together.** All N sessions launch at the same moment. That is the hardest case, and the one reported to cause MCP timeouts ([anthropics/claude-code#107472](https://github.com/anthropics/claude-code/issues/107472)).

**Two kinds of session.** Results say which one they used.

- **`sim`, a stand-in for Claude Code.** When a session starts, Claude Code does three things with each MCP server: it says hello (`initialize`), confirms (`notifications/initialized`) and asks which tools exist (`tools/list`). After that the server is ready. `sim` does exactly those three steps and nothing else. So it measures the servers alone, it gives the same numbers every time, and ten of them fit in the memory that three real sessions need.
- **`claude`, the real thing.** Real Claude Code is started the way hangar starts it (headless), with the servers passed in `--mcp-config`. The benchmark then asks Claude Code itself whether each server is connected yet (its `mcp_status` request goes from `pending` to `connected`). No message is ever sent to the model, so the benchmark uses no tokens.

**Every run starts from nothing.** Each measured run happens in a new process of its own, and a pooled run starts a brand-new hangar daemon with no servers running. So every pooled number includes starting the daemon and starting each shared server for the first time. That is the worst case for hangar. In daily use hangar is already running, and a server that earlier sessions share is already up. [Joining a pool that is already running](#joining-a-pool-that-is-already-running) measures that case.

**Memory** is private memory: what a process holds for itself and no other process can use. It is added up over every process of the run, 3 seconds after the last session became ready. Program files that several processes share are not counted twice.

**CPU** is processor time, not time on the clock. Five processes that each keep a core busy for one second have used 5 seconds of CPU, even though one second passed. Windows keeps this total for a group of processes (a "job"), and the total includes processes that have already exited. That matters here, because `npx` starts helper processes that finish within a second. The benchmark's own CPU use is subtracted.

**Ready** is the time from launching a session until all of its servers answered `tools/list` (sim) or reported `connected` (claude). A session that fails counts as failed, never as fast, and a comparison that includes a failed session is not reported.

**Leftovers** are processes still alive 5 seconds after every session ended. The pool daemon and the shared servers it keeps running on purpose are not leftovers. A per-session server that outlives its session is.

**Warm cache.** Before the first measurement, each server is started once and stopped again. So no run includes downloading a package.

**Memory guard.** The benchmark will not push your PC into swapping. Before each run it estimates how much memory the run needs, from the previous run of the same kind, or 170 MB per server per session when there is none yet. If the run would leave less than 2 GB free, it is skipped, and the table says so. A pooled run is never skipped when the direct run with the same number of sessions fit, since it needs less.
*Example.* 6 GB is free and the next run is expected to need 5 GB. Only 1 GB would remain, so the run is skipped instead of measured under memory pressure.

**Windows only.** Counting every process a run starts, including ones that already exited, needs Windows job accounting. `hangar bench` refuses to run elsewhere, so that it never reports partial numbers.

**The servers.** They are launched exactly as Claude Code requires on Windows (`cmd /c npx -y <package>`):

| Server | Package | What it does |
|---|---|---|
| Playwright | `@playwright/mcp@latest --headless --isolated` | drives a browser |
| Context7 | `@upstash/context7-mcp@latest` | looks up current library documentation |
| Memory | `@modelcontextprotocol/server-memory` | keeps notes in a knowledge graph |
| Sequential Thinking | `@modelcontextprotocol/server-sequential-thinking` | a scratchpad for step-by-step reasoning |
| Filesystem | `@modelcontextprotocol/server-filesystem <dir>` | reads and writes files in one folder |

"Five popular servers" below means all five. Playwright needs `--isolated` to serve more than one session at all: without it, a second session's browser fails with "Browser is already in use", pooled or not.

## Playwright and Context7 ([claude-pw-c7.md](results/claude-pw-c7.md))

The two local servers people add most, alone: `--preset bench/presets/playwright-context7.json`. These are real Claude Code sessions.

| Sessions | Mode | MCP processes | MCP memory | Claude memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|---:|
| 1 | direct | 11 | 340 MB | 440 MB | 3.3 s | 2.6 s |
| 1 | pooled | 8 | 149 MB | 431 MB | 2.2 s | 1.3 s |
| 1 | pooled, shared browser | 8 | 153 MB | 434 MB | 2.1 s | 1.5 s |
| 3 | direct | 33 | 1016 MB | 1.25 GB | 11.8 s | 3.4 s |
| 3 | pooled | 18 | 294 MB | 1.28 GB | 5.2 s | 1.5 s |
| 3 | pooled, shared browser | 14 | 162 MB | 1.27 GB | 4.4 s | 1.6 s |
| 5 | direct | 55 | 1.65 GB | 2.13 GB | 21.7 s | 5.2 s |
| 5 | pooled | 28 | 441 MB | 2.07 GB | 8.2 s | 2.2 s |
| 5 | pooled, shared browser | 20 | 170 MB | 2.08 GB | 6.3 s | 1.5 s |

- **With the defaults,** Context7 is one process for everyone, and each session's Playwright is a single `node` process instead of a five-process tree. 5 sessions: **1.65 GB → 441 MB (−74%)**.
- **At 1 session** there is nothing to share, so the saving comes from skipping the launcher programs alone (see [Where the savings come from](#where-the-savings-come-from-sim-no-fast-npxmd)).
- **With the shared browser on** ([claude-pw-c7-shared.md](results/claude-pw-c7-shared.md)), five sessions cost about the same as one: 170 MB (−90%).

In the table above no session has opened a page yet, so no browser is running. When every session opens a page ([sim-browse-pw-c7.md](results/sim-browse-pw-c7.md)):

| Sessions | Direct | Pooled | Pooled, shared browser ([results](results/sim-browse-pw-c7-shared.md)) |
|---:|---:|---:|---:|
| 1 | 609 MB | 415 MB | 400 MB |
| 3 | 1.78 GB | 1.08 GB (−39%) | 567 MB (−68%) |
| 5 | 2.95 GB | 1.74 GB (−41%) | 713 MB (−76%) |

By default, most of what remains is one browser per session. The shared browser replaces them with a single browser that gives each session a private context: its own tabs, cookies and storage.

Simulated sessions without browsing ([sim-pw-c7.md](results/sim-pw-c7.md)) reach 10: **3.28 GB → 794 MB (−76%)**, 100 processes → 43, tools ready in 1.3 s instead of 7.2 s.

## Five popular servers, real Claude Code ([claude.md](results/claude.md))

All five servers from the list above, in real Claude Code sessions.

| Sessions | Mode | MCP processes | MCP memory | Claude memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 837 MB | 426 MB | 8.4 s | 4.2 s |
| 1 | pooled | 17 | 355 MB | 450 MB | 3.5 s | 1.5 s |
| 1 | pooled, shared browser | 17 | 354 MB | 431 MB | 3.4 s | 1.6 s |
| 3 | direct | 78 | 2.42 GB | 1.27 GB | 25.6 s | 6.5 s |
| 3 | pooled | 41 | 755 MB | 1.30 GB | 8.2 s | 2.0 s |
| 3 | pooled, shared browser | 37 | 616 MB | 1.24 GB | 8.0 s | 2.0 s |
| 5 | direct | 130 | 3.84 GB | 2.04 GB | 42.3 s | 10.3 s |
| 5 | pooled | 65 | 1.12 GB | 2.04 GB | 13.5 s | 3.0 s |
| 5 | pooled, shared browser | 57 | 896 MB | 2.00 GB | 13.6 s | 3.5 s |

- **Three sessions:** MCP memory −70%, start-up CPU −68%.
- **Five sessions:** MCP memory **3.84 GB → 1.12 GB (−71%)**, and tools ready in 3.0 s instead of 10.3 s.
- **The whole footprint,** with Claude Code's own memory included, goes from 3.70 GB to 2.04 GB at three sessions (−45%), and from 5.88 GB to 3.16 GB at five (−46%).
- **The shared browser** ([claude-shared.md](results/claude-shared.md)) saves one Playwright process per extra session here, because no browser is running yet: 896 MB at five sessions (−77%).

## Simulated agents ([sim.md](results/sim.md))

The same five servers with the stand-in instead of Claude Code, which makes 10 sessions fit.

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 25 | 831 MB | 6.5 s | 2.5 s |
| 1 | pooled | 16 | 352 MB | 2.4 s | 806 ms |
| 5 | direct | 125 | 3.95 GB | 38.5 s | 10.6 s |
| 5 | pooled | 60 | 1.11 GB | 7.4 s | 1.9 s |
| 10 | direct | 250 | 7.38 GB | 79.3 s | 21.5 s |
| 10 | pooled | 115 | 2.08 GB | 14.6 s | 2.9 s |

Ten sessions: 7.38 GB → 2.08 GB (−72%), and tools are ready in 2.9 s instead of 21.5 s. At 1 and 5 sessions the numbers are within a few percent of the real Claude Code table, which is why the stand-in can be trusted for 10.

### Joining a pool that is already running

Every number above includes hangar starting from nothing. To measure the everyday case, these runs then started the same number of sessions again, with hangar and its shared servers already up:

| Servers | Sessions | Ready p95, starting from nothing | Ready p95, joining a running pool |
|---|---:|---:|---:|
| Playwright + Context7 ([sim-pw-c7.md](results/sim-pw-c7.md)) | 1 | 591 ms | 321 ms |
| | 5 | 976 ms | 425 ms |
| | 10 | 1.3 s | 678 ms |
| Five servers ([sim.md](results/sim.md)) | 1 | 806 ms | 445 ms |
| | 5 | 1.9 s | 1.6 s |
| | 10 | 2.9 s | 3.0 s |

A shared server that is already running answers at once. A per-session server still has to start for each new session. With two servers, one of them shared, joining takes about half the time. With five servers, three of them per-session, starting those three dominates, and at 10 sessions there is no difference.

## Every session uses its browser ([sim-browse.md](results/sim-browse.md))

Five servers again. This time each session opens a page in its browser, and checks that it sees only its own page and not another session's.

| Sessions | Mode | MCP processes | MCP memory | CPU to start | Ready p95 |
|---:|---|---:|---:|---:|---:|
| 1 | direct | 33 | 1.06 GB | 8.9 s | 2.9 s |
| 1 | pooled | 24 | 622 MB | 4.0 s | 891 ms |
| 1 | pooled, shared browser | 24 | 606 MB | 4.8 s | 1.2 s |
| 3 | direct | 99 | 3.20 GB | 29.1 s | 7.1 s |
| 3 | pooled | 62 | 1.51 GB | 12.1 s | 2.4 s |
| 3 | pooled, shared browser | 46 | 1.00 GB | 7.5 s | 1.6 s |
| 5 | direct | 165 | 5.19 GB | 49.4 s | 11.6 s |
| 5 | pooled | 100 | 2.41 GB | 20.4 s | 3.5 s |
| 5 | pooled, shared browser | 68 | 1.39 GB | 10.3 s | 2.0 s |

A running browser is several processes and about 250 MB. By default each session has one, which is why "pooled" saves less here than in the tables where no page is open. The shared-browser rows are in [sim-browse-shared.md](results/sim-browse-shared.md).

## Playwright: a browser per session, or one for all

**Why Playwright is different.** A server such as Context7 keeps nothing between requests, so hangar can pass every session's requests to one copy of it. Playwright keeps a browser with open pages, and each session needs its own. Passing every session's requests to one ordinary Playwright would mix their pages up.

**What Playwright offers instead.** It has a mode of its own for serving many clients: it listens on a local port, and with `--isolated` gives each client a private browser context inside one browser. hangar can use that mode (`share = "native"`). Of the servers hangar knows, only Playwright has such a mode, so this choice exists only for Playwright.

**Why it is off by default.** Playwright offers no password for that port. While it is open, any program on the computer, including one run by another user, could connect and drive that browser. Every other shared server talks to hangar through a private pipe with no port, behind hangar's secret. Without hangar, Playwright opens no port at all. So hangar does not open one unless you ask it to.

**When to turn it on.** On a computer only you use, the port gives other programs nothing they don't already have, since programs running as you can already read your files. There it is a fair trade for the memory.

**How to turn it on.** Add this to `~/.hangar/config.toml`, then run `hangar stop` and `hangar up`:

```toml
[servers."@playwright/mcp"]
share = "native"
```

**What it changes** (MCP memory, 5 sessions):

| | Off (default) | On | Difference |
|---|---:|---:|---:|
| Playwright + Context7, no page opened yet | 441 MB | 170 MB | −271 MB |
| Playwright + Context7, every session has a page open | 1.74 GB | 713 MB | −1.04 GB |
| Five servers, no page opened yet | 1.12 GB | 896 MB | −250 MB |
| Five servers, every session has a page open | 2.41 GB | 1.39 GB | −1.02 GB |

The setting matters most when sessions really browse: it then saves about 250 MB for each session after the first. `hangar bench --shared-browser` measures it on your machine.

## Where the savings come from ([sim-no-fast-npx.md](results/sim-no-fast-npx.md))

hangar saves memory in two separate ways. This table switches them on one at a time, for five simulated sessions with five servers:

| | MCP processes | MCP memory | CPU to start | Ready p95 |
|---|---:|---:|---:|---:|
| direct | 125 | 3.95 GB | 38.5 s | 10.6 s |
| pooled, sharing only | 111 | 2.72 GB | 25.0 s | 6.6 s |
| pooled, sharing + fast path | 60 | 1.11 GB | 7.4 s | 1.9 s |

- **Sharing.** Context7 and Memory run once for every session, instead of five times each. That alone takes 3.95 GB down to 2.72 GB.
- **The fast path.** Playwright, Sequential Thinking and Filesystem keep state per session, so they still run five times each. `npx` starts each of them through launcher programs that stay in memory. hangar starts a package that is already downloaded with `node` directly, without the launchers. That takes 2.72 GB down to 1.11 GB, and it is why hangar helps even with a single session.

**When the fast path applies.** For a package with a tag (`@latest`, or no version), hangar uses the fast path only if real `npx` checked for a newer version within the last day. The benchmark's warm-up pass is such a check, as it would be on a machine that used these servers earlier today. On the first start of a day, expect the "sharing only" row.

## Quality ([verify.md](results/verify.md))

Saving memory is worth nothing if the answers change. `hangar verify` checks that they don't:

- **Same answers.** It sends the same scripted tool calls to each server directly and through the pool, and compares the answers: 21 of 21 were identical. The pool's own request counters prove that the pooled calls really went through the pool.
- **Speed.** A call through the pool takes 0.1 ms longer (about 0.9 ms for the shared Playwright browser, which is reached over HTTP).
- **Real tasks.** Real Claude Code (Haiku) did three tasks that need MCP, three times each way: 9 of 9 succeeded both ways, with the same number of turns and the same cost.
- `verify` turns the shared Playwright browser on, so that path is checked too.

## Caveats

- **One machine.** Numbers depend on the machine, the antivirus, the npm cache and which servers you run. Run `hangar bench --preset <your .mcp.json>` to measure your own setup.
- **Each configuration ran once.** Memory repeats within a few percent between full runs. Start-up times vary more with what else the machine is doing: the five-session shared-browser run above was ready in 2.9 s on one run and 3.5 s on the next.
- **The fast path skips npm's registry check** between real npx runs (at most a day apart). `fast_npx = false` checks on every start.
- **No leftover processes either way.** Ending sessions by closing them, and by killing `claude.exe` ([claude-crash.md](results/claude-crash.md)), left no orphaned processes in either mode on this machine. hangar keeps every server in a group that Windows ends together with hangar, but there is no measured win to claim here.
- **The baseline** ([baseline-sim.md](results/baseline-sim.md)) was recorded before any pooling code existed.
