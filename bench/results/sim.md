## hangar bench: sim agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 25 | 819 MB | - | 6.0 s | 2.1 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |
| 1 | pooled | 16 | 345 MB | - | 2.0 s | 742 ms | 742 ms | 742 ms | 0 | 0 (0 MB) |
| 5 | direct | 125 | 3.96 GB | - | 36.7 s | 7.2 s | 7.4 s | 7.4 s | 0 | 0 (0 MB) |
| 5 | pooled | 52 | 845 MB | - | 5.8 s | 1.4 s | 1.4 s | 1.4 s | 0 | 0 (0 MB) |
| 10 | pooled | 97 | 1.45 GB | - | 10.0 s | 2.2 s | 2.2 s | 2.2 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 819 MB → 345 MB (**−58%**), processes 25 → 16, startup CPU 6.0 s → 2.0 s (**−67%**), ready p95 2.1 s → 742 ms, joining warm servers p50 395 ms / p95 395 ms
- **5 sessions:** MCP memory 3.96 GB → 845 MB (**−79%**), processes 125 → 52, startup CPU 36.7 s → 5.8 s (**−84%**), ready p95 7.4 s → 1.4 s, joining warm servers p50 731 ms / p95 760 ms

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
