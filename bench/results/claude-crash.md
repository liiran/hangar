## hangar bench: claude agent, preset `popular`, sessions end by killing the agent

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | direct | 78 | 2.44 GB | 1.29 GB | 25.5 s | 6.4 s | 6.4 s | 6.4 s | 0 | 0 (0 MB) |
| 3 | pooled | 41 | 752 MB | 1.24 GB | 8.5 s | 2.0 s | 2.0 s | 2.0 s | 0 | 0 (0 MB) |

- **3 sessions:** MCP memory 2.44 GB → 752 MB (**−70%**), processes 78 → 41, startup CPU 25.5 s → 8.5 s (**−67%**), ready p95 6.4 s → 2.0 s, whole session footprint 3.72 GB → 1.97 GB (−47%)

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
