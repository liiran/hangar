## hangar verify

### Tool parity (direct vs pooled)

| Server | Step | Result |
|---|---|---|
| memory | served by the pool | confirmed |
| memory | tools/list | identical |
| memory | 1. create_entities | identical |
| memory | 2. open_nodes | identical |
| memory | 3. search_nodes | identical |
| memory | 4. delete_entities | identical |
| memory | 5. read_graph | identical |
| filesystem | served by the pool | confirmed |
| filesystem | tools/list | identical |
| filesystem | 1. list_allowed_directories | identical |
| filesystem | 2. list_directory | identical |
| filesystem | 3. read_text_file | identical |
| filesystem | 4. search_files | identical |
| sequential-thinking | served by the pool | confirmed |
| sequential-thinking | tools/list | identical |
| sequential-thinking | 1. sequentialthinking | identical |
| sequential-thinking | 2. sequentialthinking | identical |
| playwright | served by the pool | confirmed |
| playwright | tools/list | identical |
| playwright | 1. browser_navigate | identical |
| playwright | 2. browser_snapshot | identical |
| playwright | 3. browser_evaluate | identical |
| playwright | 4. browser_tabs | identical |
| context7 | served by the pool | confirmed |
| context7 | tools/list | identical |
| context7 | 1. resolve-library-id | both succeeded |

**21 of 21 answers matched**, and the pool provably served the pooled calls for 5 of 5 servers. Timestamps and long numbers are masked before comparing; network answers only need to both succeed.

### Latency per tool call

| Server | Call | Direct p50 | Pooled p50 | Direct p95 | Pooled p95 |
|---|---|---:|---:|---:|---:|
| memory | read_graph | 0.38 ms | 0.49 ms | 0.64 ms | 0.81 ms |
| filesystem | list_allowed_directories | 0.12 ms | 0.22 ms | 0.24 ms | 0.38 ms |
| playwright | browser_tabs | 2.71 ms | 3.66 ms | 4.03 ms | 5.27 ms |

### Agent tasks (Claude Code, Haiku, built-in tools off)

| Task | Mode | Succeeded | Avg turns | Avg cost |
|---|---|---:|---:|---:|
| memory: store and recall | direct | 3/3 | 3.0 | $0.0003 |
| memory: store and recall | pooled | 3/3 | 3.0 | $0.0003 |
| filesystem: read a file | direct | 3/3 | 2.0 | $0.0002 |
| filesystem: read a file | pooled | 3/3 | 2.0 | $0.0002 |
| playwright: read a page | direct | 3/3 | 2.7 | $0.0004 |
| playwright: read a page | pooled | 3/3 | 2.7 | $0.0004 |
