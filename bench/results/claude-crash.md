## hangar bench: claude agent, preset `popular`, sessions end by killing the agent

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | direct | 78 | 2.38 GB | 1.24 GB | 25.5 s | 6.5 s | 6.6 s | 6.6 s | 0 | 0 (0 MB) |
| 3 | pooled | 38 | 608 MB | 1.27 GB | 8.2 s | 2.1 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |

- **3 sessions:** MCP memory 2.38 GB → 608 MB (**−75%**), processes 78 → 38, startup CPU 25.5 s → 8.2 s (**−68%**), ready p95 6.6 s → 2.1 s, whole session footprint 3.63 GB → 1.87 GB (−49%)

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
