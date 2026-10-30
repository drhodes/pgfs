# Tutorial: Performance Optimizations

!!! warning "Status: Partially implemented"
    Inode-sharded workers with their own connections, the lock order, the rename serialisation point, cross-worker flush by message, bounded buffers, prepared statements and block-granular buffered writes ship. `make bench` runs portable workloads; fio, mdtest and the passthrough-FUSE ratio run only where installed, and the CI regression gate is not built. The writeback cache stays off by design.

> The request path from the kernel to PostgreSQL is made fast where it costs most: concurrent dispatch sharded by inode, prepared statements, buffered writes and, later, the kernel's writeback cache. A fixed benchmark suite measures every change.

**Tutorial** · [How-to](../how-to/optimizationroadmap.md) · [Reference](../reference/optimizationroadmap.md) · [Explanation](../explanation/optimizationroadmap.md)

---

In this tutorial we will run the benchmark workloads, change one
setting, and compare. We need the project-local cluster.

## Step 1: run the benchmark

```bash
make bench
cat testdata/bench.json
```

The JSON names the commit, the machine and the PostgreSQL version, and
gives seconds per workload plus WAL bytes per byte of user data.

## Step 2: try one worker

```bash
PGFS_MOUNT_ARGS="--workers 1" BENCH_OUT=testdata/bench-1.json ./scripts/bench.sh
```

Compare `create_2000` and `git_clone` in the two files: with one worker
every request waits for the previous one.

## What we did

We measured the request path before and after a change, which is how
every optimisation in this roadmap is judged.

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
