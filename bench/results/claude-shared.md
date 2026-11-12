## hangar bench: claude agent, preset `popular`, one shared Playwright browser (share = "native")

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | pooled | 17 | 354 MB | 431 MB | 3.4 s | 1.6 s | 1.6 s | 1.6 s | 0 | 0 (0 MB) |
| 3 | pooled | 37 | 616 MB | 1.24 GB | 8.0 s | 2.0 s | 2.0 s | 2.0 s | 0 | 0 (0 MB) |
| 5 | pooled | 57 | 896 MB | 2.00 GB | 13.6 s | 3.3 s | 3.5 s | 3.5 s | 0 | 0 (0 MB) |

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
