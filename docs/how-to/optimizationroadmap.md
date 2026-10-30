# How-To: Performance Optimizations

!!! warning "Status: Partially implemented"
    Inode-sharded workers with their own connections, the lock order, the rename serialisation point, cross-worker flush by message, bounded buffers, prepared statements and block-granular buffered writes ship. `make bench` runs portable workloads; fio, mdtest and the passthrough-FUSE ratio run only where installed, and the CI regression gate is not built. The writeback cache stays off by design.

> The request path from the kernel to PostgreSQL is made fast where it costs most: concurrent dispatch sharded by inode, prepared statements, buffered writes and, later, the kernel's writeback cache. A fixed benchmark suite measures every change.

[Tutorial](../tutorials/optimizationroadmap.md) · **How-to** · [Reference](../reference/optimizationroadmap.md) · [Explanation](../explanation/optimizationroadmap.md)

---

## Use all the CPUs

`--workers N` sets the dispatch workers (default: the CPUs, at most
16). Each has its own database connection; the total on the primary is
`workers + 3`.

## Bound write buffering

`--buffer-per-file` (default 8 MiB) commits a file's buffer when it
grows that large; `--buffer-total` (default 256 MiB) commits the oldest
buffers when the mount's total passes it.

## Make a throwaway filesystem faster

Format it with `pgfs mkfs --scratch`: the data tables are UNLOGGED, so
nothing is written to WAL, and everything is lost on a database crash.

## Measure a change

```bash
make bench            # or BENCH_OUT=... ./scripts/bench.sh
```

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
