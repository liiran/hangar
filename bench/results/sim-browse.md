## hangar bench: sim agent, preset `popular`, every session opens a page in its browser

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 33 | 1.06 GB | - | 8.9 s | 2.9 s | 2.9 s | 2.9 s | 0 | 0 (0 MB) |
| 1 | pooled | 24 | 622 MB | - | 4.0 s | 891 ms | 891 ms | 891 ms | 0 | 0 (0 MB) |
| 3 | direct | 99 | 3.20 GB | - | 29.1 s | 6.9 s | 7.1 s | 7.1 s | 0 | 0 (0 MB) |
| 3 | pooled | 62 | 1.51 GB | - | 12.1 s | 2.4 s | 2.4 s | 2.4 s | 0 | 0 (0 MB) |
| 5 | direct | 165 | 5.19 GB | - | 49.4 s | 11.2 s | 11.6 s | 11.6 s | 0 | 0 (0 MB) |
| 5 | pooled | 100 | 2.41 GB | - | 20.4 s | 3.5 s | 3.5 s | 3.5 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 1.06 GB → 622 MB (**−43%**), processes 33 → 24, startup CPU 8.9 s → 4.0 s (**−55%**), ready p95 2.9 s → 891 ms
- **3 sessions:** MCP memory 3.20 GB → 1.51 GB (**−53%**), processes 99 → 62, startup CPU 29.1 s → 12.1 s (**−59%**), ready p95 7.1 s → 2.4 s
- **5 sessions:** MCP memory 5.19 GB → 2.41 GB (**−54%**), processes 165 → 100, startup CPU 49.4 s → 20.4 s (**−59%**), ready p95 11.6 s → 3.5 s

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
