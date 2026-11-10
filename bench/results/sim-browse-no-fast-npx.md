## hangar bench: sim agent, preset `popular`, every session opens a page in its browser, npx fast path off

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | pooled | 74 | 1.88 GB | - | 17.3 s | 4.2 s | 4.2 s | 4.2 s | 0 | 0 (0 MB) |

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
