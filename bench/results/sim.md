## hangar bench: sim agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 25 | 831 MB | - | 6.5 s | 2.5 s | 2.5 s | 2.5 s | 0 | 0 (0 MB) |
| 1 | pooled | 16 | 352 MB | - | 2.4 s | 806 ms | 806 ms | 806 ms | 0 | 0 (0 MB) |
| 5 | direct | 125 | 3.95 GB | - | 38.5 s | 10.4 s | 10.6 s | 10.6 s | 0 | 0 (0 MB) |
| 5 | pooled | 60 | 1.11 GB | - | 7.4 s | 1.9 s | 1.9 s | 1.9 s | 0 | 0 (0 MB) |
| 10 | direct | 250 | 7.38 GB | - | 79.3 s | 21.0 s | 21.5 s | 21.5 s | 0 | 0 (0 MB) |
| 10 | pooled | 115 | 2.08 GB | - | 14.6 s | 2.8 s | 2.9 s | 2.9 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 831 MB → 352 MB (**−58%**), processes 25 → 16, startup CPU 6.5 s → 2.4 s (**−63%**), ready p95 2.5 s → 806 ms, joining warm servers p50 445 ms / p95 445 ms
- **5 sessions:** MCP memory 3.95 GB → 1.11 GB (**−72%**), processes 125 → 60, startup CPU 38.5 s → 7.4 s (**−81%**), ready p95 10.6 s → 1.9 s, joining warm servers p50 1.4 s / p95 1.6 s
- **10 sessions:** MCP memory 7.38 GB → 2.08 GB (**−72%**), processes 250 → 115, startup CPU 79.3 s → 14.6 s (**−82%**), ready p95 21.5 s → 2.9 s, joining warm servers p50 2.7 s / p95 3.0 s

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
