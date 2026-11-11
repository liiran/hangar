## hangar bench: sim agent, preset `bench/presets/playwright-context7.json`

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 10 | 337 MB | - | 2.8 s | 2.3 s | 2.3 s | 2.3 s | 0 | 0 (0 MB) |
| 1 | pooled | 7 | 150 MB | - | 1.4 s | 867 ms | 867 ms | 867 ms | 0 | 0 (0 MB) |
| 5 | direct | 50 | 1.64 GB | - | 16.5 s | 6.2 s | 6.8 s | 6.8 s | 0 | 0 (0 MB) |
| 5 | pooled | 15 | 162 MB | - | 1.3 s | 863 ms | 863 ms | 863 ms | 0 | 0 (0 MB) |
| 10 | direct | 100 | 3.12 GB | - | 33.5 s | 12.0 s | 13.3 s | 13.3 s | 0 | 0 (0 MB) |
| 10 | pooled | 25 | 190 MB | - | 1.7 s | 1.7 s | 1.7 s | 1.7 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 337 MB → 150 MB (**−56%**), processes 10 → 7, startup CPU 2.8 s → 1.4 s (**−52%**), ready p95 2.3 s → 867 ms, joining warm servers p50 31 ms / p95 31 ms
- **5 sessions:** MCP memory 1.64 GB → 162 MB (**−90%**), processes 50 → 15, startup CPU 16.5 s → 1.3 s (**−92%**), ready p95 6.8 s → 863 ms, joining warm servers p50 45 ms / p95 53 ms
- **10 sessions:** MCP memory 3.12 GB → 190 MB (**−94%**), processes 100 → 25, startup CPU 33.5 s → 1.7 s (**−95%**), ready p95 13.3 s → 1.7 s, joining warm servers p50 87 ms / p95 107 ms

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
