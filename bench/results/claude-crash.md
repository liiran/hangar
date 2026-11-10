## hangar bench: claude agent, preset `popular`, sessions end by killing the agent

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | direct | 78 | 2.39 GB | 1.25 GB | 23.3 s | 5.7 s | 5.7 s | 5.7 s | 0 | 0 (0 MB) |
| 3 | pooled | 37 | 602 MB | 1.27 GB | 7.1 s | 1.7 s | 1.7 s | 1.7 s | 0 | 0 (0 MB) |

- **3 sessions:** MCP memory 2.39 GB → 602 MB (**−75%**), processes 78 → 37, startup CPU 23.3 s → 7.1 s (**−70%**), ready p95 5.7 s → 1.7 s, whole session footprint 3.64 GB → 1.85 GB (−49%)

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
