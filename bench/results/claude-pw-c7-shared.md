## hangar bench: claude agent, preset `bench/presets/playwright-context7.json`, one shared Playwright browser (share = "native")

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | pooled | 8 | 153 MB | 434 MB | 2.1 s | 1.5 s | 1.5 s | 1.5 s | 0 | 0 (0 MB) |
| 3 | pooled | 14 | 162 MB | 1.27 GB | 4.4 s | 1.6 s | 1.6 s | 1.6 s | 0 | 0 (0 MB) |
| 5 | pooled | 20 | 170 MB | 2.08 GB | 6.3 s | 1.5 s | 1.5 s | 1.5 s | 0 | 0 (0 MB) |

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
