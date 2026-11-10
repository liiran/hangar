## hangar bench: sim agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 25 | 813 MB | - | 5.6 s | 2.2 s | 2.2 s | 2.2 s | 0 | 0 (0 MB) |
| 1 | pooled | 17 | 344 MB | - | 2.4 s | 955 ms | 955 ms | 955 ms | 0 | 0 (0 MB) |
| 5 | direct | 125 | 3.94 GB | - | 36.8 s | 6.7 s | 6.9 s | 6.9 s | 0 | 0 (0 MB) |
| 5 | pooled | 53 | 846 MB | - | 5.7 s | 1.3 s | 1.3 s | 1.3 s | 0 | 0 (0 MB) |
| 10 | pooled | 98 | 1.46 GB | - | 11.3 s | 1.7 s | 1.7 s | 1.7 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 813 MB → 344 MB (**−58%**), processes 25 → 17, startup CPU 5.6 s → 2.4 s (**−56%**), ready p95 2.2 s → 955 ms, joining warm servers p50 606 ms / p95 606 ms
- **5 sessions:** MCP memory 3.94 GB → 846 MB (**−79%**), processes 125 → 53, startup CPU 36.8 s → 5.7 s (**−84%**), ready p95 6.9 s → 1.3 s, joining warm servers p50 1.1 s / p95 1.2 s

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
