# Explanation: Performance Optimizations

!!! warning "Status: Partially implemented"
    Chunked blocks, prepared statements, asynchronous commit, unlogged tables, per-handle write buffering, transaction batching and the attribute-cache TTL are implemented. Inode-keyed storage, the multi-threaded connection pool and the kernel writeback cache are not. The Reference page has the per-requirement status.

> pgfs incorporates a structured performance optimization pipeline targeting database access latency, connection concurrency, userspace buffering, and FUSE kernel-transport tuning.

[Tutorial](../tutorials/optimizationroadmap.md) · [How-to](../how-to/optimizationroadmap.md) · [Reference](../reference/optimizationroadmap.md) · **Explanation**

---

## Where the time goes

Every filesystem call on a pgfs mount crosses two boundaries.
The kernel hands the request to the daemon through `/dev/fuse`,
and the daemon turns it into one or more PostgreSQL queries
over a socket. Each query pays for a round-trip, for parsing
and planning (unless prepared), and at commit time for flushing
the write-ahead log to disk. Optimising pgfs mostly means
removing round-trips, removing parsing, and deciding how much
durability each commit buys.

## Storage layout

Storing files as 64 KiB blocks rather than one value per file
bounds the cost of a small write to one block, not the whole
file; see [Regular Files](../explanation/flat_filesystem.md).
Keying rows by path keeps them readable but makes directory
renames proportional to subtree size. The planned inode-keyed
layout trades that readability for constant-time renames.

## Fewer, cheaper queries

Prepared statements move parsing and planning to connection
time. Wrapping multi-statement operations in one transaction
replaces several commits, each with its own WAL flush, by one.
Per-handle write buffering turns a stream of small writes into
one transaction at close.

## Durability as a dial

A commit normally waits until its WAL record is on disk, which
on most storage is the slowest step of a small write. pgfs
turns that wait off (`synchronous_commit = off`). Transactions
stay atomic and the database stays consistent, but a
PostgreSQL crash can lose the last fraction of a second of
acknowledged commits. `--unlogged` goes further and skips WAL
entirely. It is fastest, but a crash empties the tables, and
unlogged tables do not replicate. Both are reasonable for
development and caches. Neither is right for data that exists
nowhere else.

## Letting the kernel answer

Tools like `ls -l`, `find` and build systems ask for file
attributes far more often than files change. With an attribute
TTL, the kernel answers repeated questions from its own cache
for that long, and pgfs never sees them. The price is
staleness for changes made outside this mount, which is why
the TTL is a flag rather than a constant.

## What is not done yet, and why

The connection pool exists, but FUSE dispatch in pgfs is
single-threaded and the filesystem object is mutated through
`&mut self`, so only one request is ever in flight. More
connections would not help until dispatch is concurrent. The
kernel's writeback cache would let the page cache merge small
writes before they reach pgfs, but it changes when and how
writes arrive (including writes from the kernel itself for
pages it caches), so it has to wait for the write path to
handle that correctly.

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
