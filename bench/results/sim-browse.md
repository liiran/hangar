## hangar bench: sim agent, preset `popular`, every session opens a page in its browser

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 33 | 1.08 GB | - | 8.0 s | 2.2 s | 2.2 s | 2.2 s | 0 | 0 (0 MB) |
| 1 | pooled | 24 | 629 MB | - | 4.0 s | 929 ms | 929 ms | 929 ms | 0 | 0 (0 MB) |
| 3 | direct | 99 | 3.20 GB | - | 28.8 s | 5.5 s | 5.6 s | 5.6 s | 0 | 0 (0 MB) |
| 3 | pooled | 46 | 1.01 GB | - | 7.7 s | 1.7 s | 1.7 s | 1.7 s | 0 | 0 (0 MB) |
| 5 | pooled | 68 | 1.43 GB | - | 10.6 s | 2.0 s | 2.0 s | 2.0 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 1.08 GB → 629 MB (**−43%**), processes 33 → 24, startup CPU 8.0 s → 4.0 s (**−50%**), ready p95 2.2 s → 929 ms
- **3 sessions:** MCP memory 3.20 GB → 1.01 GB (**−68%**), processes 99 → 46, startup CPU 28.8 s → 7.7 s (**−73%**), ready p95 5.6 s → 1.7 s

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
