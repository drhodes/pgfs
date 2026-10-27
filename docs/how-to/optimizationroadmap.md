# How-To: Performance Optimizations

!!! warning "Status: Partially implemented"
    Chunked blocks, prepared statements, asynchronous commit, unlogged tables, per-handle write buffering, transaction batching and the attribute-cache TTL are implemented. Inode-keyed storage, the multi-threaded connection pool and the kernel writeback cache are not. The Reference page has the per-requirement status.

> pgfs incorporates a structured performance optimization pipeline targeting database access latency, connection concurrency, userspace buffering, and FUSE kernel-transport tuning.

[Tutorial](../tutorials/optimizationroadmap.md) · **How-to** · [Reference](../reference/optimizationroadmap.md) · [Explanation](../explanation/optimizationroadmap.md)

---

## Cut metadata round-trips

Raise the kernel's attribute and entry cache lifetime:

```bash
pgfs --attr-ttl 10 <mountpoint>
```

Changes made through this mount are always visible to it.
Changes made by other writers (SQL, or another pgfs daemon on
the same database) can take up to the TTL to appear. Use `0`
when several writers share a database and must see each other
immediately.

## Make a throwaway mount faster

Point pgfs at a new database and pass `--unlogged`:

```bash
createdb -h "$PWD/testdata" scratch
pgfs --conn "host=$PWD/testdata dbname=scratch" --unlogged <mountpoint>
```

`--unlogged` applies only when pgfs creates the tables. To
convert existing tables, unmount and run:

```sql
ALTER TABLE entries SET UNLOGGED;
ALTER TABLE blocks SET UNLOGGED;
```

Unlogged tables are emptied after a PostgreSQL crash and are
not replicated: do not combine them with `--replica`, whose
standby would see empty tables.

## Write large files efficiently

Write through one open file and close it once. Every write on
an open handle is buffered, and closing commits all of it in
one transaction. Opening, writing and closing in a loop pays
for one transaction per iteration.

## Get full commit durability

Not configurable. pgfs sets `synchronous_commit = off` on every
connection it opens, which overrides the server's default.

## Measure the effect of a change

Compare `fuse total` from two SIGUSR1 state dumps around a
workload (see the tutorial), or the `metrics|latency` lines,
or run `./scripts/bench_profile.sh` (see [On-Demand
Profiling](ondemandprofiling.md)).

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
