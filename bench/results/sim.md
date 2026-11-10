## hangar bench: sim agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 25 | 818 MB | - | 6.0 s | 2.1 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |
| 1 | pooled | 17 | 346 MB | - | 2.2 s | 698 ms | 698 ms | 698 ms | 0 | 0 (0 MB) |
| 5 | direct | 125 | 3.95 GB | - | 36.8 s | 7.6 s | 7.8 s | 7.8 s | 0 | 0 (0 MB) |
| 5 | pooled | 53 | 872 MB | - | 6.2 s | 1.2 s | 1.2 s | 1.2 s | 0 | 0 (0 MB) |
| 10 | pooled | 98 | 1.47 GB | - | 10.2 s | 1.8 s | 1.8 s | 1.8 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 818 MB → 346 MB (**−58%**), processes 25 → 17, startup CPU 6.0 s → 2.2 s (**−64%**), ready p95 2.1 s → 698 ms, joining warm servers p50 399 ms / p95 399 ms
- **5 sessions:** MCP memory 3.95 GB → 872 MB (**−78%**), processes 125 → 53, startup CPU 36.8 s → 6.2 s (**−83%**), ready p95 7.8 s → 1.2 s, joining warm servers p50 719 ms / p95 732 ms

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
