## hangar bench: sim agent, preset `bench/presets/playwright-context7.json`

Servers: playwright, context7. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 10 | 340 MB | - | 2.5 s | 2.1 s | 2.1 s | 2.1 s | 0 | 0 (0 MB) |
| 1 | pooled | 7 | 147 MB | - | 0.9 s | 591 ms | 591 ms | 591 ms | 0 | 0 (0 MB) |
| 5 | direct | 50 | 1.64 GB | - | 15.2 s | 3.7 s | 3.8 s | 3.8 s | 0 | 0 (0 MB) |
| 5 | pooled | 23 | 435 MB | - | 3.1 s | 976 ms | 976 ms | 976 ms | 0 | 0 (0 MB) |
| 10 | direct | 100 | 3.28 GB | - | 31.4 s | 6.7 s | 7.2 s | 7.2 s | 0 | 0 (0 MB) |
| 10 | pooled | 43 | 794 MB | - | 5.3 s | 1.3 s | 1.3 s | 1.3 s | 0 | 0 (0 MB) |

- **1 sessions:** MCP memory 340 MB → 147 MB (**−57%**), processes 10 → 7, startup CPU 2.5 s → 0.9 s (**−65%**), ready p95 2.1 s → 591 ms, joining warm servers p50 321 ms / p95 321 ms
- **5 sessions:** MCP memory 1.64 GB → 435 MB (**−74%**), processes 50 → 23, startup CPU 15.2 s → 3.1 s (**−80%**), ready p95 3.8 s → 976 ms, joining warm servers p50 408 ms / p95 425 ms
- **10 sessions:** MCP memory 3.28 GB → 794 MB (**−76%**), processes 100 → 43, startup CPU 31.4 s → 5.3 s (**−83%**), ready p95 7.2 s → 1.3 s, joining warm servers p50 633 ms / p95 678 ms

Each configuration ran once, direct before pooled. Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Ready is the slowest server of each session; p95 uses the nearest-rank method. Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.
