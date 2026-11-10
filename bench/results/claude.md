## hangar bench: claude agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 816 MB | 431 MB | 6.9 s | 3.9 s | 3.9 s | 3.9 s | 0 | 0 (0 MB) |
| 1 | pooled | 18 | 347 MB | 421 MB | 3.6 s | 1.8 s | 1.8 s | 1.8 s | 0 | 0 (0 MB) |
| 3 | direct | 78 | 2.38 GB | 1.27 GB | 24.8 s | 5.3 s | 5.3 s | 5.3 s | 0 | 0 (0 MB) |
| 3 | pooled | 38 | 604 MB | 1.23 GB | 6.8 s | 2.0 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |
| 5 | direct | 130 | 3.97 GB | 2.07 GB | 43.7 s | 8.0 s | 8.0 s | 8.0 s | 0 | 0 (0 MB) |
| 5 | pooled | 58 | 870 MB | 2.14 GB | 13.2 s | 3.0 s | 3.0 s | 3.0 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 816 MB → 347 MB (**−57%**), processes 26 → 18, startup CPU 6.9 s → 3.6 s (**−48%**), ready p95 3.9 s → 1.8 s, whole session footprint 1.22 GB → 769 MB (−38%)
- **3 sessions:** MCP memory 2.38 GB → 604 MB (**−75%**), processes 78 → 38, startup CPU 24.8 s → 6.8 s (**−73%**), ready p95 5.3 s → 2.1 s, whole session footprint 3.65 GB → 1.82 GB (−50%)
- **5 sessions:** MCP memory 3.97 GB → 870 MB (**−79%**), processes 130 → 58, startup CPU 43.7 s → 13.2 s (**−70%**), ready p95 8.0 s → 3.0 s, whole session footprint 6.04 GB → 2.99 GB (−50%)

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
