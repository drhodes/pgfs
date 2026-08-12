"""
Optimization requirements and dual-interface architecture specifications for pgfs.

Derived from design/chats/chat-optimize-1.txt, design/chats/chat-rearchitect.txt,
and design/dual_interface_architecture.md.

These granular, single-responsibility Requirement classes formalize both the
existing baseline performance mechanisms in src/db.rs and the dual-interface
(Local pgrx + Remote Network) architecture specification.
"""

from .err import Feat, Req


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

    def tutorial(self):
        return (
            "In this tutorial, we will explore selecting between local in-process and remote network backends.\n"
            "1. Launch pgfs with the remote async engine: `./target/release/pgfs --backend remote --conn postgres://...`.\n"
            "2. Benchmark read/write latency using `make test-integration` or custom `fio` workloads.\n"
            "3. Build the local `pgrx` extension and start PostgreSQL postmaster with the `pgrx` worker enabled.\n"
            "4. Launch pgfs with the local engine: `./target/release/pgfs --backend local --shm /dev/shm/pgfs`.\n"
            "5. Compare latency metrics to observe sub-100µs file operation latencies."
        )

    def how_to(self):
        return (
            "To configure storage backends in pgfs:\n"
            "- For single-node maximum performance, use `--backend local` to connect via shared-memory ring buffers (`shm_mq`) to the `pgrx` background worker.\n"
            "- For networked or distributed setups, use `--backend remote` to connect via `tokio-postgres` TCP connection pools (`bb8`).\n"
            "- To configure connection pool concurrency for remote backend, pass `--max-pool-size <N>`."
        )

    def reference(self):
        return (
            "DualInterfaceArchitecture Component Reference:\n"
            "- `PgfsBackendTraitReq`: Unified async trait specifying storage engine methods (`lookup`, `read_block`, `write_block`, etc.).\n"
            "- `LocalPgrxEngineReq`: Runs inside PostgreSQL via `pgrx` postmaster background worker (`pg_bgworker`), pre-compiling SPI plans (`SPIPlanPtr`) and processing binary `Datum` values.\n"
            "- `IoUringBufferRingReq`: Uses Linux 6.9+ `fuse-io_uring` SQ/CQ rings with pre-registered buffer rings for zero-copy VFS payloads.\n"
            "- `RemoteAsyncBackendReq`: Uses `fuse3` async FUSE bindings with `tokio-postgres` connection pools (`bb8`) over TCP/Unix sockets."
        )

    def explanation(self):
        return (
            "DualInterfaceArchitecture decouples FUSE filesystem request dispatching from storage engine execution.\n"
            "The Local Engine embeds processing directly inside PostgreSQL as a background worker, bypassing SQL parsing\n"
            "and socket serialization for sub-100µs latency. The Remote Engine provides standard out-of-process networking\n"
            "using async Rust (`tokio-postgres`), allowing pgfs to scale seamlessly across network hosts."
        )


class OptimizationRoadmap(Feat):
    """pgfs incorporates a structured performance optimization pipeline
    targeting database access latency, connection concurrency, userspace
    buffering, and FUSE kernel-transport tuning."""

    def tutorial(self):
        return (
            "In this tutorial, we will apply and benchmark key pgfs performance tuning options.\n"
            "1. Start pgfs with kernel attribute caching enabled: `./target/release/pgfs --attr-ttl 5.0`.\n"
            "2. Enable writeback caching in kernel VFS: `./target/release/pgfs --writeback-cache`.\n"
            "3. Run `scripts/bench_profile.sh` to measure throughput across small-block write and read phases.\n"
            "4. Observe latency histogram metrics printed in daemon logs to evaluate p50, p95, and p99 performance."
        )

    def how_to(self):
        return (
            "To tune pgfs performance for high IOPS workloads:\n"
            "- Enable 64KB/128KB chunked block storage (`blocks(ino, block_no, data)`) to prevent full-file blob rewrites.\n"
            "- Use prepared SQL statements (`PreparedStatementReq`) to eliminate query parsing overhead on hot paths.\n"
            "- Enable synchronous commit tuning (`SET synchronous_commit = off`) to reduce WAL fsync wait times.\n"
            "- Set `--attr-ttl <seconds>` to cache inode attributes in Linux VFS and avoid repetitive `getattr` round-trips."
        )

    def reference(self):
        return (
            "OptimizationRoadmap Requirements & Parameters:\n"
            "- Schema: Chunked block storage (`ChunkedBlockStorageReq`) + integer inode primary keys (`InoBasedRekeyingReq`).\n"
            "- Database Tuning: `PreparedStatementReq`, `TransactionBatchingReq`, `SynchronousCommitTuningReq`, `UnloggedTableOptReq`.\n"
            "- Transport Tuning: `KernelAttrCacheTtlReq` (`--attr-ttl`), `KernelWritebackCacheReq` (`FUSE_WRITEBACK_CACHE`), `BufferedWriteFlushReq`.\n"
            "- Sparse Data Safety: `SparseWritePaddingReq` (pre-zero padding on uninstantiated block writes)."
        )

    def explanation(self):
        return (
            "OptimizationRoadmap addresses bottlenecks across all layers of the filesystem stack:\n"
            "database schema design (integer inode keys, chunked block storage), transaction latency (batching, WAL tuning),\n"
            "memory management (page-aligned dirty write buffering), and kernel VFS interaction (attribute TTL caching,\n"
            "writeback cache coalescing). Together, these optimizations deliver near-native filesystem throughput."
        )
