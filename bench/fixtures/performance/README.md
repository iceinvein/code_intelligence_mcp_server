# Direct MCP performance fixtures

These fixtures measure the server itself. They do not invoke an answering
agent or an LLM judge, so their latency can be compared independently from the
quality benchmark.

Prerequisites:

- Run the release daemon and index the fixture repository first.
- Keep the daemon configuration fixed for the full comparison.
- Set `CODE_INTEL_PERF_LARGE_REPO` to a checked-out Django repository for the
  large fixture.

Example (run only when a performance measurement is intended):

```bash
python3 scripts/profile_direct_mcp.py \
  bench/fixtures/performance/direct-mcp-medium.json \
  --iterations 30 \
  --output bench/results/P001-medium.json
```

The report records both repository and server Git revisions, live server
configuration returned by `get_index_stats`, index entity counts, cold latency,
warm p50/p95/p99, and response size for search, investigate, definition,
references, and impact traversal. Optional `max_warm_p95_ms` values on fixture
operations turn the profiler into a failing regression gate once stable
baselines have been measured.
