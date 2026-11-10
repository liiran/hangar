## hangar bench: sim agent, preset `popular`, every session opens a page in its browser

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 33 | 1.07 GB | - | 8.5 s | 2.6 s | 2.6 s | 2.6 s | 0 | 0 (0 MB) |
| 1 | pooled | 25 | 629 MB | - | 4.6 s | 987 ms | 987 ms | 987 ms | 0 | 0 (0 MB) |
| 3 | direct | 99 | 3.20 GB | - | 29.5 s | 5.9 s | 6.0 s | 6.0 s | 0 | 0 (0 MB) |
| 3 | pooled | 47 | 1.03 GB | - | 7.7 s | 1.6 s | 1.6 s | 1.6 s | 0 | 0 (0 MB) |
| 5 | direct | 165 | 5.10 GB | - | 50.8 s | 10.3 s | 11.3 s | 11.3 s | 0 | 0 (0 MB) |
| 5 | pooled | 69 | 1.43 GB | - | 11.4 s | 2.7 s | 2.7 s | 2.7 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 1.07 GB → 629 MB (**−43%**), processes 33 → 25, startup CPU 8.5 s → 4.6 s (**−46%**), ready p95 2.6 s → 987 ms
- **3 sessions:** MCP memory 3.20 GB → 1.03 GB (**−68%**), processes 99 → 47, startup CPU 29.5 s → 7.7 s (**−74%**), ready p95 6.0 s → 1.6 s
- **5 sessions:** MCP memory 5.10 GB → 1.43 GB (**−72%**), processes 165 → 69, startup CPU 50.8 s → 11.4 s (**−78%**), ready p95 11.3 s → 2.7 s

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
