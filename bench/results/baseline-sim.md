## hangar bench: sim agent, preset `popular`

Servers: playwright, context7, memory, sequential-thinking, filesystem. Machine: 31.06 GB RAM, 12 logical CPUs, windows.

| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | direct | 25 | 816 MB | - | 6.3 s | 2.3 s | 2.3 s | 2.3 s | 0 | 0 (0 MB) |
| 5 | direct | 125 | 3.84 GB | - | 38.8 s | 10.8 s | 10.9 s | 10.9 s | 0 | 0 (0 MB) |

Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. CPU comes from job-object accounting and includes processes that exited during startup. Leftovers are processes still alive 5 s after every session closed; a pool daemon and the servers it keeps warm on purpose are excluded.
