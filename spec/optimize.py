"""
Optimization requirements and dual-interface architecture specifications for pgfs.

Derived from design/chats/chat-optimize-1.txt, design/chats/chat-rearchitect.txt,
and design/dual_interface_architecture.md.

These granular, single-responsibility Requirement classes formalize both the
existing baseline performance mechanisms in src/db.rs and the dual-interface
(Local pgrx + Remote Network) architecture specification.
"""

from .err import PARTIAL, PLANNED, Feat, Req, md


# ── Tier 1 — Database Path & Schema Optimizations ──────────────────────

class ChunkedBlockStorageReq(Req):
    """File contents are stored in fixed-size block chunks (64KB or 128KB) in a
    `blocks(ino, block_no, data)` table rather than monolithic byte blobs in
    `entries.data`.

    Technical Details:
    - Read operations calculate block ranges `[start_block..end_block]` based on
      `offset` and `size`, fetching only touched blocks from PostgreSQL.
    - Write operations update or upsert only touched block records rather than
      rewriting the entire file payload.
    - Truncation via `setattr` deletes block records where `block_no > new_size / block_size`
      and trims trailing block bytes.
    - Metadata (`size`, `mtime`, `ctime`) remains in `entries` as the source of truth."""
    pass


class InoBasedRekeyingReq(Req):
    """Rekey `entries` and `blocks` tables from `(parent, name)` text keys to
    64-bit integer `ino BIGSERIAL` primary keys (`entries(ino)` and
    `blocks(ino, block_no)`).

    Technical Details:
    - `entries` table uses `ino BIGSERIAL PRIMARY KEY` and `parent_ino BIGINT REFERENCES entries(ino)`.
    - Directory `rename(old_parent, old_name, new_parent, new_name)` updates `parent_ino = new_parent`
      and `name = new_name` on a single row in $O(1)$ time, eliminating $O(N)$ cascading string updates.
    - Path lookup is accelerated via B-Tree index on `(parent_ino, name)`.
    - Inode numbers are guaranteed stable for open file handles and kernel VFS stat callbacks."""
    pass


class SparseWritePaddingReq(Req):
    """Unaligned or sparse block writes (where `offset_in_block > 0` on an
    uninstantiated block record) zero-pad preceding bytes at the application layer
    before upserting to prevent byte-0 data corruption.

    Technical Details:
    - When `write_block(ino, block_no, data, offset_in_block)` is called for a block
      that does not yet exist in `blocks`, the engine checks if `offset_in_block > 0`.
    - If `offset_in_block > 0`, the engine constructs a zero-filled byte array of length
      `offset_in_block` and prepends it to `data` before upserting.
    - Subsequent reads of `[0..offset_in_block]` correctly return zero-filled bytes
      conforming to POSIX sparse file behavior."""
    pass


class ReplicaFreshnessCacheReq(Req):
    """When running with `--replica`, the replica freshness check
    (`replica_fresh`) caches its health and WAL LSN verdict using an atomic TTL cache
    to eliminate per-read validation overhead.

    Technical Details:
    - Uses `AtomicBool` for freshness state and `AtomicU64` for timestamp tracking.
    - Re-evaluates Postgres physical streaming standby lag (`pg_is_in_recovery`,
      `pg_current_wal_lsn`, `pg_last_wal_replay_lsn`) only when `now - last_checked > 500ms`.
    - Consecutive read operations reuse the cached verdict, reducing query overhead by 75%
      while ensuring automatic fallback to primary if lag exceeds tolerance."""
    pass


class PreparedStatementReq(Req):
    """All static SQL queries (getattr, read, write, create, mkdir, unlink,
    rmdir, list, rename) are pre-compiled as prepared statements upon
    database connection initialization.

    Technical Details:
    - Pre-compiles statement handles (`stmt_getattr`, `stmt_read_blocks`, `stmt_upsert_block`, etc.)
      during `DbPool::get()` or worker initialization.
    - Eliminates SQL lexical parsing, semantic analysis, and query planning overhead from
      the FUSE operation hot path."""
    pass


class SynchronousCommitTuningReq(Req):
    """The database layer supports connection-level tuning of
    `synchronous_commit` (`SET synchronous_commit = off`) to optimize per-operation
    fsync latency.

    Technical Details:
    - Executed during connection acquisition (`on_acquire` hook in connection pool manager).
    - Allows transactions to return immediately after writing to Postgres WAL buffers,
      trading a sub-second crash durability window for up to 10x higher metadata IOPS."""
    pass


class UnloggedTableOptReq(Req):
    """The database schema supports creating `entries` and `blocks` as
    `UNLOGGED` tables for high-performance development, caching, and testing workloads.

    Technical Details:
    - DDL specifies `CREATE UNLOGGED TABLE entries` and `CREATE UNLOGGED TABLE blocks`.
    - Bypasses WAL logging entirely, maximizing raw write throughput for ephemeral mounts."""
    pass


class TransactionBatchingReq(Req):
    """FUSE operations that execute multiple sequential database queries
    wrap them in a single explicit database transaction.

    Technical Details:
    - Groups multi-query operations (e.g. truncate + write, directory deletion checks)
      within `BEGIN` ... `COMMIT` blocks.
    - Reduces network round-trips and WAL commit fsync overhead from $N$ commits to 1 commit."""
    pass


# ── Tier 2 — Dual-Interface Architecture ──────────────────────────────

class PgfsBackendTraitReq(Req):
    """A unified async `PgfsBackend` trait decouples the FUSE driver
    (`src/fs.rs`) from storage engine implementations.

    Technical Details:
    - Exposes async methods: `lookup`, `getattr`, `setattr`, `readdir`, `mkdir`, `rmdir`,
      `create`, `unlink`, `rename`, `read_block`, `write_block`, `truncate_blocks`.
    - Maps database failures into structured `StorageError` variants (`NotFound`, `AlreadyExists`,
      `NotEmpty`, `InvalidArgument`, `DatabaseError`, `IoError`).
    - Enables pluggable backend switching (`LocalBackend` vs `RemoteBackend`) at daemon startup."""
    pass


class LocalPgrxEngineReq(Req):
    """The local storage engine (`LocalBackend`) operates inside PostgreSQL as
    a `pgrx` background worker (`pg_bgworker`) using pre-compiled SPI plans and binary Datums
    to achieve Zero SQL Text Parsing on the hot path.

    Technical Details:
    - Runs inside PostgreSQL postmaster via `RegisterBackgroundWorker`.
    - Pre-compiles statement handles (`SPIPlanPtr`) during worker startup using `SPI_prepare`.
    - Receives binary operation descriptors over a lockless shared-memory ring buffer (`shm_mq`)
      from the standalone FUSE daemon.
    - Executes `SPI_execute_plan` using raw binary `Datum` values (`Int64GetDatum`, `Int32GetDatum`),
      bypassing SQL text formatting, parsing, and planning entirely."""
    pass


class IoUringBufferRingReq(Req):
    """Where supported by kernel (Linux 6.9+ `fuse-io_uring`), pgfs utilizes
    `io_uring` Submission/Completion queues paired with pre-registered shared
    memory buffer rings (`IORING_OP_PROVIDE_BUFFERS`) for zero-copy data flow.

    Technical Details:
    - Replaces `/dev/fuse` `read()`/`write()` syscalls with asynchronous `io_uring` SQ/CQ rings.
    - Maps POSIX shared memory buffers (`shm_mq`) as pre-registered `io_uring` buffer rings.
    - Kernel VFS places incoming file block payloads directly into shared memory buffers accessible by
      the `pgrx` worker, enabling end-to-end zero-copy data flow into PostgreSQL `SharedBuffers`."""
    pass


class RemoteAsyncBackendReq(Req):
    """The remote storage engine (`RemoteBackend`) operates out-of-process using
    `fuse3` and `tokio-postgres` over TCP or Unix sockets.

    Technical Details:
    - Uses `fuse3` async FUSE bindings and `tokio-postgres` connection pools (`bb8`).
    - Implements query pipelining over single connection handles using `query_raw`.
    - Integrates atomic `ReplicaFreshnessCache` for read-replica traffic routing."""
    pass


class MultiThreadedConnectionPoolReq(Req):
    """The database client uses a thread-safe connection pool (`r2d2` or `bb8`),
    combined with multi-worker FUSE session dispatch to process parallel operations concurrently.

    Technical Details:
    - Configures pool size via CLI `--max-pool-size`.
    - Replaces single-threaded `&mut self` Db handles with pooled connection acquisition per thread."""
    pass


class BufferedWriteFlushReq(Req):
    """Small or non-aligned FUSE `write` calls buffer dirty byte ranges
    or blocks in memory per open file handle, flushing pending writes
    to PostgreSQL on `flush`, `release`, or explicit `fsync`.

    Technical Details:
    - Maintains a per-fd dirty block buffer in daemon memory.
    - Aggregates page-sized kernel writes into full 128KB block writes before issuing database upserts."""
    pass


# ── Tier 3 — FUSE & Kernel Layer ──────────────────────────────────────

class KernelAttrCacheTtlReq(Req):
    """The kernel attribute and entry cache TTL is configurable via a CLI
    option (`--attr-ttl`), reducing repetitive `lookup` and `getattr` FUSE callbacks.

    Technical Details:
    - Sets `TTL` duration in `fuser::ReplyAttr` and `fuser::ReplyEntry` responses.
    - Bypasses daemon and database queries for stat/lookup ops on unchanged paths during TTL window."""
    pass


class KernelWritebackCacheReq(Req):
    """FUSE callback initialization negotiates kernel capabilities including
    `FUSE_WRITEBACK_CACHE` and splice read/write.

    Technical Details:
    - Enables `CAP_WRITEBACK_CACHE` in `KernelConfig` during FUSE `init`.
    - Allows Linux VFS page cache to coalesce small sequential write syscalls into 128KB block buffers."""
    pass


# ── Features ──────────────────────────────────────────────────────────

class DualInterfaceArchitecture(Feat):
    """pgfs features a Dual-Interface Storage Architecture providing a sub-100µs
    Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy)
    and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining)."""

    deps = [
        PgfsBackendTraitReq,
        LocalPgrxEngineReq,
        IoUringBufferRingReq,
        RemoteAsyncBackendReq,
    ]

    def title(self):
        return "Dual-Interface Architecture"

    def status(self):
        return PLANNED

    def status_note(self):
        return (
            "The `PgfsBackend` trait, its error mapping and an inode-keyed "
            "implementation on `Db` exist and are unit-tested, but the FUSE "
            "layer does not route through them yet. `RemoteBackend` is a "
            "stub, the `pgrx` engine and the `io_uring` transport are at the "
            "design stage, and there is no `--backend` option."
        )

    def tutorial(self):
        return md(r"""
            The dual-interface architecture is not wired into the daemon
            yet, so this tutorial explores the part that exists: the
            storage seam that both future engines will plug into. We will
            run its tests, read the trait, and see where the FUSE layer
            will connect to it. All we need is the repository and the
            project-local cluster.

            ## Step 1: run the backend tests

            ```bash
            ./scripts/ensure_db.sh
            cargo test backend
            ```

            ```text
            test backend::tests::test_storage_error_to_libc_errno ... ok
            test backend::tests::test_remote_backend_unimplemented_stubs ... ok
            test backend::tests::test_file_kind_conversion ... ok
            test backend::tests::test_prepare_sparse_block_write_overwrite_existing ... ok
            test backend::tests::test_prepare_sparse_block_write_zero_padding ... ok
            test db::tests_offload::test_backend_offloaded_copy_file_range_and_fallocate ... ok
            test result: ok. 6 passed; 0 failed; ...
            ```

            ## Step 2: read the trait

            ```bash
            grep -n 'async fn' src/backend.rs | head -16
            ```

            Every method takes inode numbers rather than paths. That is the
            most important difference from the code that runs today, which
            keys everything by `(parent path, name)`.

            ## Step 3: see how errors become errnos

            ```bash
            grep -n -A12 'impl From<StorageError> for libc::c_int' src/backend.rs
            ```

            A backend reports what went wrong (`NotFound`, `NotEmpty`,
            `DatabaseError`, ...), and one function turns that into the
            errno the kernel receives.

            ## Step 4: find the seam in the FUSE layer

            ```bash
            grep -n 'backend' src/fs.rs | head
            ```

            `PgFs` already holds a `backend` field, constructed as a
            `LocalBackend` around the same `Db`, but the callbacks still
            call `self.db` directly. Switching each callback from `self.db`
            to `self.backend` is the next step of this feature.

            ## What we did

            We exercised and read the storage abstraction that will let
            pgfs choose between an in-process engine and a network engine.
            The [explanation](../explanation/dualinterfacearchitecture.md)
            describes where it is heading.
        """)

    def how_to(self):
        return md(r"""
            ## Choose a backend at startup

            Not available yet. There is no `--backend` option; the daemon
            always uses the in-process `Db` client over libpq.

            ## Add an operation to the storage interface

            1. Add an `async fn` to `PgfsBackend` in `src/backend.rs`. If
               not every backend can support it, give it a default body
               that returns `StorageError::InvalidArgument`, as
               `copy_file_range` and `fallocate` do.
            2. Implement it for `Db` in `db::backend_impl` (`src/db.rs`).
            3. Forward it in `impl PgfsBackend for LocalBackend`.
            4. Add a stub to `RemoteBackend`.
            5. Add a test next to `tests_offload` in `src/db.rs`.

            ## Report a storage failure with the right errno

            Return the matching `StorageError` variant and convert it with
            `libc::c_int::from(err)` when replying to the kernel. Use
            `DatabaseError` (→ `EIO`) only for failures that are not a
            normal answer, and log those with their full context.

            ## Run only the backend tests

            ```bash
            cargo test backend
            ```

            ## Follow the design

            The full design, including the shared-memory protocol and the
            `io_uring` data path, is in
            `design/dual_interface_architecture.md`.
        """)

    def reference(self):
        return md(r"""
            ## Component status

            | Component | Status | Location |
            |---|---|---|
            | `PgfsBackend` trait | implemented, not used by the FUSE layer | `src/backend.rs` |
            | `StorageError` → errno mapping | implemented | `src/backend.rs` |
            | inode-keyed implementation on `Db` | implemented; exercised by unit tests only | `src/db.rs`, `backend_impl` |
            | `LocalBackend` | wrapper that forwards to `Db`; not the planned `pgrx` engine | `src/backend.rs` |
            | `RemoteBackend` | stub; every method returns an error | `src/backend.rs` |
            | `pgrx` background-worker engine (`LocalPgrxEngineReq`) | planned | design only |
            | `io_uring` buffer rings (`IoUringBufferRingReq`) | planned | design only |
            | `tokio-postgres` network engine (`RemoteAsyncBackendReq`) | planned | design only |
            | `--backend` command-line option | planned | |

            ## `PgfsBackend` methods

            | Method | Signature (all `async`, all return `Result<_, StorageError>`) |
            |---|---|
            | `lookup` | `(parent_ino: u64, name: &str) -> Option<FileAttr>` |
            | `getattr` | `(ino: u64) -> FileAttr` |
            | `setattr` | `(ino: u64, attr: SetAttrInput) -> FileAttr` |
            | `readdir` | `(ino: u64, offset: i64) -> Vec<DirEntry>` |
            | `mkdir` | `(parent_ino: u64, name: &str, mode: u32, uid: u32, gid: u32) -> FileAttr` |
            | `rmdir` | `(parent_ino: u64, name: &str) -> ()` |
            | `create` | `(parent_ino: u64, name: &str, mode: u32, uid: u32, gid: u32) -> FileAttr` |
            | `unlink` | `(parent_ino: u64, name: &str) -> ()` |
            | `rename` | `(old_parent: u64, old_name: &str, new_parent: u64, new_name: &str) -> ()` |
            | `read_block` | `(ino: u64, block_no: u32) -> Vec<u8>` |
            | `write_block` | `(ino: u64, block_no: u32, data: &[u8], offset_in_block: u32) -> u32` |
            | `truncate_blocks` | `(ino: u64, new_size: u64) -> ()` |
            | `copy_file_range` | `(ino_in, off_in, ino_out, off_out, len: u64) -> u64`; default: `InvalidArgument` |
            | `fallocate` | `(ino: u64, offset: u64, length: u64, mode: u32) -> ()`; default: `InvalidArgument` |

            ## `StorageError`

            | Variant | errno |
            |---|---|
            | `NotFound` | `ENOENT` |
            | `AlreadyExists` | `EEXIST` |
            | `NotEmpty` | `ENOTEMPTY` |
            | `InvalidArgument(String)` | `EINVAL` |
            | `DatabaseError(String)` | `EIO` |
            | `IoError(kind)` | `ENOENT`, `EEXIST`, `EACCES` or `EINVAL` for the matching kinds; otherwise `EIO` |

            ## Data types

            | Type | Fields |
            |---|---|
            | `FileKind` | `File`, `Directory`, `Symlink` |
            | `FileAttr` | `ino`, `size`, `blocks`, `atime`, `mtime`, `ctime`, `kind`, `perm`, `nlink`, `uid`, `gid`, `rdev`, `flags` |
            | `DirEntry` | `ino`, `name`, `kind` |
            | `SetAttrInput` | optional `mode`, `uid`, `gid`, `size`, `atime`, `mtime` |

            ## Compatibility with the running FUSE path

            | Property | FUSE path (`Db` path methods) | Backend path (`Db` as `PgfsBackend`) |
            |---|---|---|
            | Key | `(parent, name)` text | `ino`, `parent_ino` integers |
            | Block size | 65 536 bytes | 131 072 bytes |
            | `entries.parent_ino` written | always 1 | the real parent |
            | `blocks.ino` written | always 1 | the real inode |

            Rows written by one path are not correctly visible through the
            other, so the two cannot yet serve the same database.
        """)

    def explanation(self):
        return md(r"""
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
        """)


class OptimizationRoadmap(Feat):
    """pgfs incorporates a structured performance optimization pipeline
    targeting database access latency, connection concurrency, userspace
    buffering, and FUSE kernel-transport tuning."""

    deps = [
        ChunkedBlockStorageReq,
        InoBasedRekeyingReq,
        SparseWritePaddingReq,
        ReplicaFreshnessCacheReq,
        PreparedStatementReq,
        SynchronousCommitTuningReq,
        UnloggedTableOptReq,
        TransactionBatchingReq,
        MultiThreadedConnectionPoolReq,
        BufferedWriteFlushReq,
        KernelAttrCacheTtlReq,
        KernelWritebackCacheReq,
    ]

    def title(self):
        return "Performance Optimizations"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "Chunked blocks, prepared statements, asynchronous commit, "
            "unlogged tables, per-handle write buffering, transaction "
            "batching and the attribute-cache TTL are implemented. "
            "Inode-keyed storage, the multi-threaded connection pool and "
            "the kernel writeback cache are not. The Reference page has the "
            "per-requirement status."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will mount pgfs on a scratch database with
            two tuning options, `--unlogged` and `--attr-ttl`, and measure
            how many requests the kernel sends with and without attribute
            caching. We need the project-local cluster running
            (`./scripts/ensure_db.sh`).

            ## Step 1: create a scratch database and mount it

            ```bash
            createdb -h "$PWD/testdata" pgfs_tuning
            mkdir -p /tmp/pgfs-tuning
            ./target/debug/pgfs --conn "host=$PWD/testdata dbname=pgfs_tuning" \
                --unlogged --attr-ttl 0 /tmp/pgfs-tuning 2> /tmp/pgfs-tuning.log &
            sleep 1
            ```

            Because the database is new, `--unlogged` creates both tables
            without write-ahead logging:

            ```bash
            psql -h "$PWD/testdata" -d pgfs_tuning -Atc \
              "SELECT relname, relpersistence FROM pg_class WHERE relname IN ('entries', 'blocks')"
            ```

            ```text
            entries|u
            blocks|u
            ```

            ## Step 2: create some files

            ```bash
            mkdir -p /tmp/pgfs-tuning/tree/{a,b,c,d,e}
            for d in a b c d e; do for i in $(seq 1 20); do echo x > /tmp/pgfs-tuning/tree/$d/f$i; done; done
            ```

            ## Step 3: count requests with no attribute cache

            We use the state dump's `fuse total`, the number of FUSE
            requests served so far. Take one reading, walk the tree three
            times, and take another:

            ```bash
            PID=$(pgrep -f 'pgfs --conn.*pgfs_tuning')
            dump() { kill -USR1 $PID; ls /tmp/pgfs-tuning > /dev/null; sleep 0.3;
                     grep -o 'fuse_latency=fuse total=[0-9]*' /tmp/pgfs-tuning.log | tail -1; }
            dump
            for k in 1 2 3; do ls -lR /tmp/pgfs-tuning/tree > /dev/null; done
            dump
            ```

            With `--attr-ttl 0` the second total is about 4,200 higher: the
            kernel asked pgfs about every entry, every time.

            ## Step 4: repeat with a 30-second cache

            ```bash
            kill -TERM $PID; sleep 1
            ./target/debug/pgfs --conn "host=$PWD/testdata dbname=pgfs_tuning" \
                --attr-ttl 30 /tmp/pgfs-tuning 2> /tmp/pgfs-tuning.log &
            sleep 1
            PID=$(pgrep -f 'pgfs --conn.*pgfs_tuning')
            dump
            for k in 1 2 3; do ls -lR /tmp/pgfs-tuning/tree > /dev/null; done
            dump
            ```

            This time the difference is about 160. The first walk asked
            pgfs; the next two were answered from the kernel's cache.

            ## Step 5: clean up

            ```bash
            kill -TERM $PID; sleep 1
            dropdb -h "$PWD/testdata" pgfs_tuning
            rm -f /tmp/pgfs-tuning.log
            ```

            ## What we did

            We created unlogged storage for a throwaway mount and cut the
            requests for a metadata-heavy workload by more than 95% with
            one flag. The [explanation](../explanation/optimizationroadmap.md)
            covers what each option costs.
        """)

    def how_to(self):
        return md(r"""
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
        """)

    def reference(self):
        return md(r"""
            ## Command-line options

            | Option | Default | Effect |
            |---|---|---|
            | `--attr-ttl <seconds>` | `1.0` | TTL attached to every attribute and entry reply; fractional values allowed |
            | `--unlogged` | off | create `entries` and `blocks` as `UNLOGGED` tables; no effect on existing tables |

            ## Built-in settings

            | Setting | Value |
            |---|---|
            | Block size (FUSE path) | 65 536 bytes |
            | `synchronous_commit` | `off`, set on every pooled connection |
            | Connection pool | `r2d2`, max 10 per pool; the daemon holds 1 primary connection (plus 1 standby connection) for its lifetime |
            | FUSE dispatch | single-threaded |
            | Kernel capabilities requested | `FUSE_BIG_WRITES` |
            | Replica freshness cache | 500 ms |
            | Prepared on connect | `getattr`, `create`, `mkdir`, `unlink`, `rmdir`, `list`, `read_blocks`, `upsert_block`, `prune_blocks`, `delete_blocks`, and six inode-keyed statements |

            Replica queries, `rename`, size updates and `rmdir`'s emptiness
            check use unprepared SQL.

            ## Requirement status

            | Requirement | Status | Notes |
            |---|---|---|
            | `ChunkedBlockStorageReq` | implemented | 64 KiB blocks in `blocks(parent, name, block_no)` |
            | `InoBasedRekeyingReq` | partial | `ino`/`parent_ino` columns and prepared statements exist; the FUSE path is still keyed by path and leaves them at their defaults |
            | `SparseWritePaddingReq` | partial | unbuffered writes merge with the stored block; writes buffered on an open handle do not, see `BufferedWriteFlushReq` |
            | `ReplicaFreshnessCacheReq` | implemented | 500 ms monotonic TTL |
            | `PreparedStatementReq` | implemented | hot-path primary statements; see list above |
            | `SynchronousCommitTuningReq` | implemented | always on; not configurable |
            | `UnloggedTableOptReq` | implemented | `--unlogged` |
            | `TransactionBatchingReq` | implemented | `write_range`, `truncate`, the buffered flush and `rename` are single transactions; `unlink` and `rmdir` are separate statements |
            | `MultiThreadedConnectionPoolReq` | partial | the pool exists but one connection is used and dispatch is single-threaded; no `--max-pool-size` |
            | `BufferedWriteFlushReq` | implemented | per open handle; flushed on `flush`, `fsync`, `release` and before a `read` of the same file; buffered blocks start empty rather than from the stored block |
            | `KernelAttrCacheTtlReq` | implemented | `--attr-ttl` |
            | `KernelWritebackCacheReq` | not implemented | only `FUSE_BIG_WRITES` is negotiated; no `--writeback-cache` |
        """)

    def explanation(self):
        return md(r"""
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
        """)
