## hangar bench: claude agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 821 MB | 436 MB | 6.6 s | 3.6 s | 3.6 s | 3.6 s | 0 | 0 (0 MB) |
| 1 | pooled | 18 | 350 MB | 437 MB | 3.1 s | 1.5 s | 1.5 s | 1.5 s | 0 | 0 (0 MB) |
| 3 | direct | 78 | 2.39 GB | 1.29 GB | 25.2 s | 5.8 s | 5.8 s | 5.8 s | 0 | 0 (0 MB) |
| 3 | pooled | 38 | 607 MB | 1.24 GB | 7.6 s | 1.8 s | 1.8 s | 1.8 s | 0 | 0 (0 MB) |
| 5 | direct | 130 | 3.63 GB | 2.09 GB | 47.0 s | 10.5 s | 11.0 s | 11.0 s | 0 | 0 (0 MB) |
| 5 | pooled | 58 | 883 MB | 2.10 GB | 12.4 s | 2.9 s | 3.0 s | 3.0 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 821 MB → 350 MB (**−57%**), processes 26 → 18, startup CPU 6.6 s → 3.1 s (**−53%**), ready p95 3.6 s → 1.5 s, whole session footprint 1.23 GB → 786 MB (−37%)
- **3 sessions:** MCP memory 2.39 GB → 607 MB (**−75%**), processes 78 → 38, startup CPU 25.2 s → 7.6 s (**−70%**), ready p95 5.8 s → 1.8 s, whole session footprint 3.68 GB → 1.83 GB (−50%)
- **5 sessions:** MCP memory 3.63 GB → 883 MB (**−76%**), processes 130 → 58, startup CPU 47.0 s → 12.4 s (**−74%**), ready p95 11.0 s → 3.0 s, whole session footprint 5.72 GB → 2.96 GB (−48%)

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
