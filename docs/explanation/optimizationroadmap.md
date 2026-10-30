# Explanation: Performance Optimizations

!!! warning "Status: Partially implemented"
    Inode-sharded workers with their own connections, the lock order, the rename serialisation point, cross-worker flush by message, bounded buffers, prepared statements and block-granular buffered writes ship. `make bench` runs portable workloads; fio, mdtest and the passthrough-FUSE ratio run only where installed, and the CI regression gate is not built. The writeback cache stays off by design.

> The request path from the kernel to PostgreSQL is made fast where it costs most: concurrent dispatch sharded by inode, prepared statements, buffered writes and, later, the kernel's writeback cache. A fixed benchmark suite measures every change.

[Tutorial](../tutorials/optimizationroadmap.md) · [How-to](../how-to/optimizationroadmap.md) · [Reference](../reference/optimizationroadmap.md) · **Explanation**

---

## Where the time goes

A FUSE request crosses the kernel, the daemon and PostgreSQL. With a
single dispatch thread every request waits for the one before it, so
the first gain is concurrency: workers sharded by inode, each with its
own connection, so requests on different files run in parallel while
requests on one file stay in order.

## Fewer, cheaper queries

Each operation is one call to a prepared `pgfs.*` function, so the
database parses and plans each statement once per connection, and the
daemon never reads a row just to decide which statement to send next.

## Buffered writes

Small writes collect in a per-inode buffer and commit as whole blocks,
so a burst of 4 KiB writes becomes a few blob inserts. A partly
written block is read and overlaid first, which is both the fix for
Bug 1 and the reason a checksum can be taken once per blob.

## What is not done yet

The kernel's writeback cache is the largest single win left on the
data path, and stays off until its consequences for mtime, appends and
fsync are covered by crash tests. A benchmark regression gate in CI is
the other missing piece.

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
