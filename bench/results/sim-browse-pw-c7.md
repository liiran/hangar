## hangar bench: sim agent, preset `bench/presets/playwright-context7.json`, every session opens a page in its browser

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 18 | 609 MB | - | 4.2 s | 2.1 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |
| 1 | pooled | 15 | 415 MB | - | 3.0 s | 763 ms | 763 ms | 763 ms | 0 | 0 (0 MB) |
| 3 | direct | 54 | 1.78 GB | - | 15.3 s | 3.8 s | 3.9 s | 3.9 s | 0 | 0 (0 MB) |
| 3 | pooled | 39 | 1.08 GB | - | 8.5 s | 1.9 s | 1.9 s | 1.9 s | 0 | 0 (0 MB) |
| 5 | direct | 90 | 2.95 GB | - | 26.1 s | 5.3 s | 5.7 s | 5.7 s | 0 | 0 (0 MB) |
| 5 | pooled | 63 | 1.74 GB | - | 13.6 s | 2.2 s | 2.2 s | 2.2 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 609 MB → 415 MB (**−32%**), processes 18 → 15, startup CPU 4.2 s → 3.0 s (**−28%**), ready p95 2.1 s → 763 ms
- **3 sessions:** MCP memory 1.78 GB → 1.08 GB (**−39%**), processes 54 → 39, startup CPU 15.3 s → 8.5 s (**−45%**), ready p95 3.9 s → 1.9 s
- **5 sessions:** MCP memory 2.95 GB → 1.74 GB (**−41%**), processes 90 → 63, startup CPU 26.1 s → 13.6 s (**−48%**), ready p95 5.7 s → 2.2 s

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
