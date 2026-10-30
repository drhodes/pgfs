# Explanation: Dual-Interface Architecture

!!! warning "Status: Partially implemented"
    The FUSE layer reaches storage only through the synchronous `PgfsBackend` trait. `PgBackend` (tokio-postgres, one connection per worker, LSN tickets pipelined in the same round trip) is the remote engine, and the reference model in `tests/model/` implements the same trait. The pgrx engine waits on its measurement condition and the io_uring transport on crate support.

> Two storage engines sit behind one interface: a planned local engine that runs inside PostgreSQL and sends no SQL text on the hot path, and a planned remote engine that pipelines queries over the network. The FUSE layer talks only to the interface, so the engine is chosen at startup.

[Tutorial](../tutorials/dualinterfacearchitecture.md) · [How-to](../how-to/dualinterfacearchitecture.md) · [Reference](../reference/dualinterfacearchitecture.md) · **Explanation**

---

## Two very different places to run

pgfs is used in two settings with opposite constraints. On a single
machine the database is local, and the dominant costs are the overheads
around each query: protocol framing, parsing and copying. Over a
network a round trip costs far more than any of that, and what matters
is sending fewer of them: pipelining, batching and replicas.

One engine cannot be optimal for both. The dual-interface design gives
each its own engine behind one interface.

## The remote engine, built

`PgBackend` is an ordinary out-of-process client built on
`tokio-postgres`. Each dispatch worker owns one connection for its
whole life, prepares its statements once, and runs each operation as
one call to a `pgfs.*` function, so the number of connections per mount
is fixed and each operation is one round trip. Reads may go to a
standby under the read tickets of [Replicated
Storage](replicated_filesystem.md).

## The local engine, planned

A local engine would run inside PostgreSQL as a `pgrx` background
worker, executing the same `pgfs.*` functions from pre-planned
statements with binary arguments. It is justified only if a benchmark
shows socket and protocol overhead still dominating after round trips
and concurrency were dealt with, and only once the blast radius of a
crashing background worker (a postmaster restart) is contained.

## Why a trait

The `PgfsBackend` trait makes the FUSE layer speak inode-shaped
operations, and puts the error model in one place. It is also what lets
the reference model stand in for the database in tests.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
