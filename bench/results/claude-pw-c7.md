## hangar bench: claude agent, preset `bench/presets/playwright-context7.json`

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 11 | 340 MB | 440 MB | 3.3 s | 2.6 s | 2.6 s | 2.6 s | 0 | 0 (0 MB) |
| 1 | pooled | 8 | 149 MB | 431 MB | 2.2 s | 1.3 s | 1.3 s | 1.3 s | 0 | 0 (0 MB) |
| 3 | direct | 33 | 1016 MB | 1.25 GB | 11.8 s | 3.4 s | 3.4 s | 3.4 s | 0 | 0 (0 MB) |
| 3 | pooled | 18 | 294 MB | 1.28 GB | 5.2 s | 1.5 s | 1.5 s | 1.5 s | 0 | 0 (0 MB) |
| 5 | direct | 55 | 1.65 GB | 2.13 GB | 21.7 s | 5.0 s | 5.2 s | 5.2 s | 0 | 0 (0 MB) |
| 5 | pooled | 28 | 441 MB | 2.07 GB | 8.2 s | 2.1 s | 2.2 s | 2.2 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 340 MB → 149 MB (**−56%**), processes 11 → 8, startup CPU 3.3 s → 2.2 s (**−34%**), ready p95 2.6 s → 1.3 s, whole session footprint 780 MB → 580 MB (−26%)
- **3 sessions:** MCP memory 1016 MB → 294 MB (**−71%**), processes 33 → 18, startup CPU 11.8 s → 5.2 s (**−56%**), ready p95 3.4 s → 1.5 s, whole session footprint 2.24 GB → 1.57 GB (−30%)
- **5 sessions:** MCP memory 1.65 GB → 441 MB (**−74%**), processes 55 → 28, startup CPU 21.7 s → 8.2 s (**−62%**), ready p95 5.2 s → 2.2 s, whole session footprint 3.77 GB → 2.50 GB (−34%)

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
