# Explanation: Dual-Interface Architecture

!!! danger "Status: Planned, not implemented"
    The `PgfsBackend` trait, its error mapping and an inode-keyed implementation on `Db` exist and are unit-tested, but the FUSE layer does not route through them yet. `RemoteBackend` is a stub, the `pgrx` engine and the `io_uring` transport are at the design stage, and there is no `--backend` option.

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy) and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

[Tutorial](../tutorials/dualinterfacearchitecture.md) · [How-to](../how-to/dualinterfacearchitecture.md) · [Reference](../reference/dualinterfacearchitecture.md) · **Explanation**

---

## Two very different places to run

pgfs is used in two settings with opposite constraints. On a
single machine, the database is local and the dominant costs
are the overheads around each query: building SQL text,
parsing and planning it, and copying bytes between the kernel,
the daemon and PostgreSQL. Over a network, a round-trip costs
far more than any of that, and what matters is sending fewer
of them: pipelining, batching and replicas.

One engine cannot be optimal for both. The dual-interface
design gives each its own engine behind one interface.

## The local engine: no SQL on the hot path

The planned local engine runs inside PostgreSQL itself, as a
background worker written with `pgrx`. The FUSE daemon passes
it compact binary requests over a shared-memory queue, and the
worker executes pre-planned statements with binary parameters,
so no SQL text is formatted, parsed or planned per operation.
With `io_uring` buffer rings, file data could travel from the
kernel into memory the worker reads directly, without
intermediate copies. The target is operation latency under
100 µs.

## The remote engine: fewer round-trips

The planned remote engine is an ordinary out-of-process client
built on `tokio-postgres`. It pipelines queries over each
connection, uses a connection pool, and reuses the replica
freshness cache from [Replicated
Storage](replicated_filesystem.md). It suits mounts where the
database is on another host.

## Why a trait comes first

Both engines need the FUSE layer to stop speaking SQL-shaped
paths and start speaking inode-shaped operations. The
`PgfsBackend` trait is that contract. It also forces the error
model into the open: a backend reports `NotFound` or
`DatabaseError`, and one place decides which errno the kernel
sees. Building the trait and an inode-keyed implementation
first, with tests, makes the eventual switch-over a mechanical
change in `fs.rs` rather than a rewrite.

## What stands in the way

The running FUSE path stores data keyed by path, with 64 KiB
blocks, and leaves the inode columns at their defaults. The
backend implementation expects real inode links and uses
128 KiB blocks. Moving the FUSE layer onto the trait therefore
also means migrating the schema
(`InoBasedRekeyingReq` in [Performance
Optimizations](optimizationroadmap.md)) and settling on one
block size.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
