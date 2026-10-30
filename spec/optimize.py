"""
Performance: the request path from the kernel through the daemon to
PostgreSQL, and the dual-interface storage architecture.

Derived from design/chats/chat-optimize-1.txt,
design/chats/chat-rearchitect.txt and
design/dual_interface_architecture.md. The concurrency requirements live
in `concurrency.py` and the benchmarks in `perf.py`; the
`OptimizationRoadmap` feature documents them together.
"""

from .concurrency import (
    BoundedWriteBuffers,
    InodeShardedWorkers,
    LockOrder,
    NoLockAcrossRoundTrip,
    RenameSerialisationPoint,
    WorkerOwnsConnection,
)
from .err import PARTIAL, Feat, Req, md
from .consistency import (
    DrainsBuffersBeforeExternalising,
)
from .durability import (
    FsyncFailureSemantics,
)
from .failure import (
    HonoursInterrupt,
)
from .posix import (
    WritebackCacheDisabled,
)
from .perf import BenchmarkRegressionGate, BenchmarkSuite


# ── Tier 1 — Database Path & Schema Optimizations ──────────────────────

class PreparedStatementReq(Req):
    """Every single-statement read the daemon issues (lookup, getattr,
    readdir pages, block reads, xattr reads) and every call to a `pgfs`
    function is prepared once per connection, when the worker opens it
    (`concurrency.WorkerOwnsConnection`). The hot path never sends SQL
    text to be parsed and planned. Because each worker owns its
    connection for life, a statement is never prepared lazily, and never
    prepared twice.

    **Bounds.** Parsing and planning happen once per statement per
    connection.
    """

    deps = [WorkerOwnsConnection]


# ── Tier 2 — Dual-Interface Architecture ──────────────────────────────

class PgfsBackendTraitReq(Req):
    """The `PgfsBackend` trait (`src/backend.rs`) is the only way the
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

    **Sources.** ShardStore r128."""

class LocalPgrxEngineReq(Req):
    """`LocalBackend` runs inside PostgreSQL as a `pgrx` background worker
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
      packaging: an extension, not a fork."""

    deps = [PgfsBackendTraitReq, BenchmarkSuite]


class IoUringBufferRingReq(Req):
    """Where the kernel and the FUSE crate support it, pgfs uses FUSE
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

    **Sources.** RFUSE r17."""


class RemoteAsyncBackendReq(Req):
    """`RemoteBackend` (`PgBackend` in `src/pg.rs`) runs out of process,
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

    **Sources.** DynamoDB transactions r82, DepFast r87, FiDe r78."""

    deps = [PgfsBackendTraitReq, WorkerOwnsConnection, HonoursInterrupt]


class BufferedWriteFlushReq(Req):
    """Writes are buffered in a single per-inode write buffer owned by
    the inode's worker (`concurrency.InodeShardedWorkers`). Every write
    to an inode advances a per-inode monotonic `write_seq` and records
    the handle's `last_write_seq` and `seen_err_seq` (`errseq_t`,
    `durability.FsyncFailureSemantics`). The inode buffer is committed
    as a unit by `flush`, `release`, `fsync`, any operation that names
    the file (`consistency.DrainsBuffersBeforeExternalising`), and
    buffer pressure (`concurrency.BoundedWriteBuffers`). A write of a
    whole aligned block, or an `O_SYNC` / `O_DSYNC` / `O_DIRECT` write,
    commits any overlapping dirty buffered data (or drains the inode
    buffer first) so an earlier buffered byte can never overwrite a
    later direct or aligned write (`durability.SyncHandles`).

    Buffers are block-granular, at the filesystem's block size
    (`storage.FixedBlockSize`), with dirty byte intervals (or a dirty
    bitmap) per block. Committing a partly dirty block reads the
    committed block first and overlays the dirty intervals on it. A
    block with nothing committed starts as zeros. This read-modify-write
    rule is the fix for Bug 1.

    **Postcondition.** After commit, the file equals the committed
    content with every buffered write applied in arrival order, across
    all open descriptors on the inode.

    **Concurrency.** Every read, getattr and listing on the mount sees
    the buffered data (`consistency.DrainsBuffersBeforeExternalising`).

    **Errors.** A failed commit keeps the uncommitted data in the inode
    buffer and reports once per descriptor that wrote (`last_write_seq`)
    via `errseq_t` (`durability.FsyncFailureSemantics`).

    **Sources.** Skyros r129, r90."""

    deps = [InodeShardedWorkers, BoundedWriteBuffers, DrainsBuffersBeforeExternalising, FsyncFailureSemantics]


# ── Tier 3 — FUSE & Kernel Layer ──────────────────────────────────────

class KernelWritebackCacheReq(Req):
    """The kernel writeback cache (`FUSE_WRITEBACK_CACHE`) is the largest
    single win on the data path (To FUSE or Not r12). It stays off
    (`posix.WritebackCacheDisabled`) until each of these holds and is
    covered by a crash test:

    - mtime and size are owned by the kernel while the cache is on, and
      the daemon accepts them from setattr;
    - writes that arrive after `release`, from pages the kernel flushes
      later, are accepted and committed;
    - `O_APPEND` positioning is left to the kernel;
    - `fsync` covers pages still in the kernel, because the kernel
      flushes them before calling the daemon;
    - `CrashConformance` runs a matrix with the cache on.

    Splice for reads and writes is negotiated with the cache.

    **Sources.** To FUSE or Not r12, r77."""

    deps = [WritebackCacheDisabled]


# ── Features ──────────────────────────────────────────────────────────

class DualInterfaceArchitecture(Feat):
    """Two storage engines sit behind one interface: a planned local engine
    that runs inside PostgreSQL and sends no SQL text on the hot path, and
    a planned remote engine that pipelines queries over the network. The
    FUSE layer talks only to the interface, so the engine is chosen at
    startup."""

    deps = [
        PgfsBackendTraitReq,
        LocalPgrxEngineReq,
        IoUringBufferRingReq,
        RemoteAsyncBackendReq,
    ]

    def title(self):
        return "Dual-Interface Architecture"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "The FUSE layer reaches storage only through the synchronous "
            "`PgfsBackend` trait. `PgBackend` (tokio-postgres, one "
            "connection per worker, LSN tickets pipelined in the same "
            "round trip) is the remote engine, and the reference model in "
            "`tests/model/` implements the same trait. The pgrx engine "
            "waits on its measurement condition and the io_uring "
            "transport on crate support."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will drive the same storage interface the daemon
            uses from two implementations: the PostgreSQL backend and the
            in-memory reference model, and watch them agree. We need the
            project-local cluster (see [Your first pgfs mount](projectlocalenvironment.md)).

            ## Step 1: run the backend tests

            ```bash
            cargo test --test storage
            ```

            Every test formats a fresh database, drives `PgBackend` through the
            `PgfsBackend` trait, and ends with `pgfs check`.

            ## Step 2: run the model against the database

            ```bash
            cargo test --test model_conformance
            ```

            The suite generates operation sequences, runs each against
            `tests/model/` and against PostgreSQL through the same trait, and
            compares replies and whole trees after every step.

            ## Step 3: read the trait

            Open `src/backend.rs`. Each method is one `pgfs.*` SQL function or
            prepared read; the FUSE layer in `src/fs.rs` calls nothing else.

            ## What we did

            We saw one interface with two implementations that the daemon cannot
            tell apart, which is what lets pgfs add a second engine later without
            touching the FUSE layer.
        """)

    def how_to(self):
        return md(r"""
            ## Add an operation to the storage interface

            1. Write the `pgfs.*` SQL function in `sql/v1/` (a new migration for a
               released schema).
            2. Add the method to `PgfsBackend` in `src/backend.rs`.
            3. Implement it in `PgBackend` (`src/pg.rs`): prepare the statement in
               `statements()` and call it through `mutate` or `read`.
            4. Implement it in the reference model (`tests/model/mod.rs`).
            5. Add the operation to the model-conformance generator.

            ## Report a storage failure with the right errno

            Raise `PF0xx` from SQL (the codes are listed at the top of
            `sql/v1/10_core.sql`); `src/errno.rs` maps every SQLSTATE in one table.

            ## Run only the backend tests

            ```bash
            cargo test --test storage --test model_conformance
            cargo test --features fault-injection --test fault_injection
            ```
        """)

    def reference(self):
        return md(r"""
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
        """)

    def explanation(self):
        return md(r"""
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
        """)

class OptimizationRoadmap(Feat):
    """The request path from the kernel to PostgreSQL is made fast where it
    costs most: concurrent dispatch sharded by inode, prepared
    statements, buffered writes and, later, the kernel's writeback cache.
    A fixed benchmark suite measures every change."""

    deps = [
        InodeShardedWorkers,
        WorkerOwnsConnection,
        LockOrder,
        RenameSerialisationPoint,
        NoLockAcrossRoundTrip,
        BoundedWriteBuffers,
        PreparedStatementReq,
        BufferedWriteFlushReq,
        KernelWritebackCacheReq,
        BenchmarkSuite,
        BenchmarkRegressionGate,
    ]

    def title(self):
        return "Performance Optimizations"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "Inode-sharded workers with their own connections, the lock "
            "order, the rename serialisation point, cross-worker flush by "
            "message, bounded buffers, prepared statements and "
            "block-granular buffered writes ship. `make bench` runs "
            "portable workloads; fio, mdtest and the passthrough-FUSE "
            "ratio run only where installed, and the CI regression gate "
            "is not built. The writeback cache stays off by design."
        )

    def tutorial(self):
        return md(r"""
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
        """)

    def how_to(self):
        return md(r"""
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
        """)

    def reference(self):
        return md(r"""
            ## Options

            | Option | Default | Effect |
            |---|---|---|
            | `--workers` | CPUs, at most 16 | inode-sharded dispatch workers |
            | `--buffer-per-file` | 8 MiB | commit a file's buffer at this size |
            | `--buffer-total` | 256 MiB | commit the oldest buffers past this total |
            | `--attr-ttl` | 3 600 s read-write, 1 s read-only | kernel entry and attribute TTL |

            ## Status

            | Requirement | State |
            |---|---|
            | `InodeShardedWorkers`, `WorkerOwnsConnection`, `NoLockAcrossRoundTrip` | built |
            | `LockOrder`, `RenameSerialisationPoint` | built, in the SQL functions |
            | `BoundedWriteBuffers`, `BufferedWriteFlushReq`, `PreparedStatementReq` | built |
            | `KernelWritebackCacheReq` | off by design until its preconditions are covered by crash tests |
            | `BenchmarkSuite` | portable workloads in `scripts/bench.sh`; fio, mdtest and the passthrough ratio where installed |
            | `BenchmarkRegressionGate` | not built |
        """)

    def explanation(self):
        return md(r"""
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
        """)
