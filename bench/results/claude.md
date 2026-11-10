## hangar bench: claude agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 820 MB | 438 MB | 6.9 s | 3.9 s | 3.9 s | 3.9 s | 0 | 0 (0 MB) |
| 1 | pooled | 17 | 346 MB | 430 MB | 2.8 s | 1.4 s | 1.4 s | 1.4 s | 0 | 0 (0 MB) |
| 3 | direct | 78 | 2.39 GB | 1.24 GB | 24.3 s | 6.1 s | 6.1 s | 6.1 s | 0 | 0 (0 MB) |
| 3 | pooled | 37 | 599 MB | 1.27 GB | 6.9 s | 1.8 s | 1.8 s | 1.8 s | 0 | 0 (0 MB) |
| 5 | pooled | 57 | 857 MB | 1.95 GB | 11.8 s | 2.8 s | 2.9 s | 2.9 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 820 MB → 346 MB (**−58%**), processes 26 → 17, startup CPU 6.9 s → 2.8 s (**−60%**), ready p95 3.9 s → 1.4 s, whole session footprint 1.23 GB → 775 MB (−38%)
- **3 sessions:** MCP memory 2.39 GB → 599 MB (**−76%**), processes 78 → 37, startup CPU 24.3 s → 6.9 s (**−72%**), ready p95 6.1 s → 1.8 s, whole session footprint 3.64 GB → 1.85 GB (−49%)

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
