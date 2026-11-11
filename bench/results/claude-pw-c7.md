## hangar bench: claude agent, preset `bench/presets/playwright-context7.json`

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 11 | 336 MB | 432 MB | 3.7 s | 2.7 s | 2.7 s | 2.7 s | 0 | 0 (0 MB) |
| 1 | pooled | 8 | 151 MB | 430 MB | 2.1 s | 1.5 s | 1.5 s | 1.5 s | 0 | 0 (0 MB) |
| 3 | direct | 33 | 1012 MB | 1.27 GB | 13.4 s | 4.7 s | 4.8 s | 4.8 s | 0 | 0 (0 MB) |
| 3 | pooled | 14 | 158 MB | 1.25 GB | 4.9 s | 2.2 s | 2.2 s | 2.2 s | 0 | 0 (0 MB) |
| 5 | direct | 55 | 1.64 GB | 1.96 GB | 21.9 s | 6.7 s | 7.0 s | 7.0 s | 0 | 0 (0 MB) |
| 5 | pooled | 20 | 169 MB | 2.05 GB | 6.4 s | 1.8 s | 1.9 s | 1.9 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 336 MB → 151 MB (**−55%**), processes 11 → 8, startup CPU 3.7 s → 2.1 s (**−41%**), ready p95 2.7 s → 1.5 s, whole session footprint 767 MB → 581 MB (−24%)
- **3 sessions:** MCP memory 1012 MB → 158 MB (**−84%**), processes 33 → 14, startup CPU 13.4 s → 4.9 s (**−64%**), ready p95 4.8 s → 2.2 s, whole session footprint 2.26 GB → 1.41 GB (−38%)
- **5 sessions:** MCP memory 1.64 GB → 169 MB (**−90%**), processes 55 → 20, startup CPU 21.9 s → 6.4 s (**−71%**), ready p95 7.0 s → 1.9 s, whole session footprint 3.60 GB → 2.21 GB (−39%)

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
