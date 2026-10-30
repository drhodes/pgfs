# Reference: Dual-Interface Architecture

!!! warning "Status: Partially implemented"
    The FUSE layer reaches storage only through the synchronous `PgfsBackend` trait. `PgBackend` (tokio-postgres, one connection per worker, LSN tickets pipelined in the same round trip) is the remote engine, and the reference model in `tests/model/` implements the same trait. The pgrx engine waits on its measurement condition and the io_uring transport on crate support.

> Two storage engines sit behind one interface: a planned local engine that runs inside PostgreSQL and sends no SQL text on the hot path, and a planned remote engine that pipelines queries over the network. The FUSE layer talks only to the interface, so the engine is chosen at startup.

[Tutorial](../tutorials/dualinterfacearchitecture.md) · [How-to](../how-to/dualinterfacearchitecture.md) · **Reference** · [Explanation](../explanation/dualinterfacearchitecture.md)

---

## Components

| Component | State | Where |
|---|---|---|
| `PgfsBackend` trait | built; synchronous, one backend per worker | `src/backend.rs` |
| `PgBackend` (remote engine) | built: tokio-postgres, prepared statements, pipelined LSN tickets, idempotent mutations | `src/pg.rs` |
| reference model | built | `tests/model/mod.rs` |
| `pgrx` local engine | planned, once a benchmark shows protocol overhead dominates | — |
| `io_uring` transport | waits on FUSE crate support | — |

## Errors

| `FsError` | Reply |
|---|---|
| `Errno(e)` | `e`, logged at DEBUG at most |
| `RetryDirty(inos)` | none: the worker flushes the inodes and calls again |
| `Fenced` | EIO; the mount is fenced |
| `Unavailable` | EIO; the connection is not Connected |
| `Unexpected` | EIO (or the stated errno), logged with the full chain |


## Specification requirements

`DualInterfaceArchitecture` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "PgfsBackendTraitReq"
    `spec.optimize.PgfsBackendTraitReq`

    The `PgfsBackend` trait (`src/backend.rs`) is the only way the
    FUSE layer (`src/fs.rs`) reaches storage. It is synchronous: each
    dispatch worker owns one backend and calls it from its own thread
    (`concurrency.WorkerOwnsConnection`).

    - Each method corresponds to one `pgfs.*` SQL function or prepared
      read (`storage.OperationsAsSqlFunctions`). Reads take the view
      `(layer, epoch, root)`; mutations take the layer, and the backend
      adds the fence and the idempotency key
      `(mount_id, worker_id, op_seq)` (`failure.IdempotentMutations`).
    - Block reads and writes are batched (`read_blocks`, `write_blocks`).
    - The trait covers the whole POSIX and tree surface: `lookup`,
      `getattr`, `readdir`, `readdirplus`, `read_blocks`, `readlink`,
      `getxattr`, `listxattr`, `seek_block`, `statfs`, `make` (create,
      mkdir, mknod, symlink), `link`, `unlink`, `rmdir`, `rename`,
      `setattr`, `write_blocks`, `copy_range`, `setxattr`,
      `removexattr`, `release_orphan`, `sync_barrier`, `snapshot`,
      `branch` and `promote`.

    Failures are `FsError` values: an expected errno from
    `posix.ErrnoCatalogue`, a request to flush dirty inodes and call
    again, fencing, an unavailable database, or an unexpected failure
    with its error chain (`failure.MapsDatabaseErrors`).

    Today `PgBackend` (the remote engine) is the only database backend;
    once a local engine exists, the daemon will choose between them at
    startup. The in-memory reference model (`verify.ReferenceModel`)
    implements the same trait, and the daemon cannot tell it from a real
    backend.

    **Sources.** ShardStore r128.

??? abstract "LocalPgrxEngineReq"
    `spec.optimize.LocalPgrxEngineReq`

    `LocalBackend` runs inside PostgreSQL as a `pgrx` background worker
    (`RegisterBackgroundWorker`), so the hot path sends no SQL text:

    - the FUSE daemon passes it binary operation descriptors over a
      shared-memory queue (`shm_mq`);
    - at startup the worker prepares `SPIPlanPtr` handles with
      `SPI_prepare` for the same `pgfs.*` functions that `RemoteBackend`
      calls over the wire (`storage.OperationsAsSqlFunctions`), so
      mutation logic, lock order (`concurrency.LockOrder`), epochs and
      fencing exist once, not once per engine;
    - it runs them with `SPI_execute_plan` and binary `Datum` arguments,
      with no SQL formatting, parsing or planning per operation.

    Two conditions apply before this is built:

    - **Measure first.** Disaggregation research (r134) says to
      co-locate only where the measured communication cost justifies
      it. Round trips (`consistency.OneRoundTripPerOperation`) and
      concurrent dispatch (`concurrency.InodeShardedWorkers`) are the
      larger costs, and are removed first. This engine is justified only
      if a benchmark (`perf.BenchmarkSuite`) shows that socket
      and protocol overhead still dominates after them.
    - **State the blast radius.** A crashing background worker restarts
      the whole postmaster, which takes every connection to every
      database on the cluster with it. The requirement must say how
      that risk is contained before it is built. Citus (r39) shows the
      packaging: an extension, not a fork.

??? abstract "IoUringBufferRingReq"
    `spec.optimize.IoUringBufferRingReq`

    Where the kernel and the FUSE crate support it, pgfs uses FUSE
    over io_uring, which was merged in Linux 6.14:

    - per-core submission and completion rings replace `read()` and
      `write()` on `/dev/fuse`;
    - registered buffer rings put request payloads in pre-mapped daemon
      memory;
    - without kernel or crate support, the daemon falls back to
      `/dev/fuse` and logs which transport is in use.

    The `fuser` crate pgfs uses (0.14) has no io_uring support. This
    requirement waits on crate support, or on a move to a crate that has
    it.

    The data still crosses one more copy than "zero-copy" suggests:
    Postgres copies every block into a heap or TOAST tuple. The gain is
    in fewer syscalls and context switches, not fewer copies.

    **Sources.** RFUSE r17.

??? abstract "RemoteAsyncBackendReq"
    `spec.optimize.RemoteAsyncBackendReq`

    `RemoteBackend` (`PgBackend` in `src/pg.rs`) runs out of process,
    using `tokio-postgres` over TCP or a Unix socket on a
    current-thread runtime owned by each worker:

    - each worker owns one connection for its whole life, opened at
      startup (`concurrency.WorkerOwnsConnection`). There is no
      connection pool, and no connection is opened on demand;
    - statements are prepared once per connection, and a mutation and
      its LSN ticket query are pipelined on the connection, so each
      operation stays one round trip
      (`consistency.OneRoundTripPerOperation`);
    - reads go to a standby only under an LSN ticket
      (`replica.LsnTicketReads`).

    Every operation has an explicit timeout (`failure.BoundsEveryWait`)
    and can be cancelled from another thread with the connection's
    cancel token: the watchdog cancels a stuck query, and a FUSE
    INTERRUPT will once the FUSE crate delivers it
    (`failure.HonoursInterrupt`).

    **Sources.** DynamoDB transactions r82, DepFast r87, FiDe r78.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
