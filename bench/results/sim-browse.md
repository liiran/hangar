## hangar bench: sim agent, preset `popular`, every session opens a page in its browser

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 33 | 1.08 GB | - | 8.1 s | 2.1 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |
| 1 | pooled | 25 | 608 MB | - | 4.1 s | 1.1 s | 1.1 s | 1.1 s | 0 | 0 (0 MB) |
| 3 | direct | 99 | 3.20 GB | - | 28.9 s | 4.8 s | 5.3 s | 5.3 s | 0 | 0 (0 MB) |
| 3 | pooled | 47 | 1.02 GB | - | 7.1 s | 1.2 s | 1.2 s | 1.2 s | 0 | 0 (0 MB) |
| 5 | direct | 165 | 5.13 GB | - | 50.4 s | 9.2 s | 9.3 s | 9.3 s | 0 | 0 (0 MB) |
| 5 | pooled | 69 | 1.44 GB | - | 10.6 s | 1.9 s | 1.9 s | 1.9 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 1.08 GB → 608 MB (**−45%**), processes 33 → 25, startup CPU 8.1 s → 4.1 s (**−49%**), ready p95 2.1 s → 1.1 s
- **3 sessions:** MCP memory 3.20 GB → 1.02 GB (**−68%**), processes 99 → 47, startup CPU 28.9 s → 7.1 s (**−75%**), ready p95 5.3 s → 1.2 s
- **5 sessions:** MCP memory 5.13 GB → 1.44 GB (**−72%**), processes 165 → 69, startup CPU 50.4 s → 10.6 s (**−79%**), ready p95 9.3 s → 1.9 s

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
