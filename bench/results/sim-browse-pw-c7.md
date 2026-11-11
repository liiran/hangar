## hangar bench: sim agent, preset `bench/presets/playwright-context7.json`, every session opens a page in its browser

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 18 | 610 MB | - | 5.4 s | 3.4 s | 3.4 s | 3.4 s | 0 | 0 (0 MB) |
| 1 | pooled | 15 | 400 MB | - | 3.0 s | 994 ms | 994 ms | 994 ms | 0 | 0 (0 MB) |
| 3 | direct | 54 | 1.78 GB | - | 16.9 s | 6.0 s | 6.0 s | 6.0 s | 0 | 0 (0 MB) |
| 3 | pooled | 23 | 555 MB | - | 4.7 s | 1.7 s | 1.7 s | 1.7 s | 0 | 0 (0 MB) |
| 5 | direct | 90 | 2.86 GB | - | 27.8 s | 7.6 s | 8.2 s | 8.2 s | 0 | 0 (0 MB) |
| 5 | pooled | 31 | 718 MB | - | 6.1 s | 1.8 s | 1.8 s | 1.8 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 610 MB → 400 MB (**−34%**), processes 18 → 15, startup CPU 5.4 s → 3.0 s (**−44%**), ready p95 3.4 s → 994 ms
- **3 sessions:** MCP memory 1.78 GB → 555 MB (**−69%**), processes 54 → 23, startup CPU 16.9 s → 4.7 s (**−72%**), ready p95 6.0 s → 1.7 s
- **5 sessions:** MCP memory 2.86 GB → 718 MB (**−75%**), processes 90 → 31, startup CPU 27.8 s → 6.1 s (**−78%**), ready p95 8.2 s → 1.8 s

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
