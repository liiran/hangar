## hangar bench: claude agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 26 | 837 MB | 426 MB | 8.4 s | 4.2 s | 4.2 s | 4.2 s | 0 | 0 (0 MB) |
| 1 | pooled | 17 | 355 MB | 450 MB | 3.5 s | 1.5 s | 1.5 s | 1.5 s | 0 | 0 (0 MB) |
| 3 | direct | 78 | 2.42 GB | 1.27 GB | 25.6 s | 6.3 s | 6.5 s | 6.5 s | 0 | 0 (0 MB) |
| 3 | pooled | 41 | 755 MB | 1.30 GB | 8.2 s | 1.9 s | 2.0 s | 2.0 s | 0 | 0 (0 MB) |
| 5 | direct | 130 | 3.84 GB | 2.04 GB | 42.3 s | 10.0 s | 10.3 s | 10.3 s | 0 | 0 (0 MB) |
| 5 | pooled | 65 | 1.12 GB | 2.04 GB | 13.5 s | 2.9 s | 3.0 s | 3.0 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 837 MB → 355 MB (**−58%**), processes 26 → 17, startup CPU 8.4 s → 3.5 s (**−59%**), ready p95 4.2 s → 1.5 s, whole session footprint 1.23 GB → 805 MB (−36%)
- **3 sessions:** MCP memory 2.42 GB → 755 MB (**−70%**), processes 78 → 41, startup CPU 25.6 s → 8.2 s (**−68%**), ready p95 6.5 s → 2.0 s, whole session footprint 3.70 GB → 2.04 GB (−45%)
- **5 sessions:** MCP memory 3.84 GB → 1.12 GB (**−71%**), processes 130 → 65, startup CPU 42.3 s → 13.5 s (**−68%**), ready p95 10.3 s → 3.0 s, whole session footprint 5.88 GB → 3.16 GB (−46%)

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
