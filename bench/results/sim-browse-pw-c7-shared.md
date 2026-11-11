## hangar bench: sim agent, preset `bench/presets/playwright-context7.json`, every session opens a page in its browser, one shared Playwright browser (share = "native")

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | pooled | 15 | 400 MB | - | 2.6 s | 913 ms | 913 ms | 913 ms | 0 | 0 (0 MB) |
| 3 | pooled | 23 | 567 MB | - | 4.1 s | 1.2 s | 1.2 s | 1.2 s | 0 | 0 (0 MB) |
| 5 | pooled | 31 | 713 MB | - | 5.0 s | 1.6 s | 1.6 s | 1.6 s | 0 | 0 (0 MB) |

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
