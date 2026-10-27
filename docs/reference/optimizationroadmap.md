# Reference: Performance Optimizations

!!! warning "Status: Partially implemented"
    Chunked blocks, prepared statements, asynchronous commit, unlogged tables, per-handle write buffering, transaction batching and the attribute-cache TTL are implemented. Inode-keyed storage, the multi-threaded connection pool and the kernel writeback cache are not. The Reference page has the per-requirement status.

> pgfs incorporates a structured performance optimization pipeline targeting database access latency, connection concurrency, userspace buffering, and FUSE kernel-transport tuning.

[Tutorial](../tutorials/optimizationroadmap.md) · [How-to](../how-to/optimizationroadmap.md) · **Reference** · [Explanation](../explanation/optimizationroadmap.md)

---

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


## Specification requirements

`OptimizationRoadmap` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ChunkedBlockStorageReq"
    `spec.optimize.ChunkedBlockStorageReq`

    File contents are stored in fixed-size block chunks (64KB or 128KB) in a
    `blocks(ino, block_no, data)` table rather than monolithic byte blobs in
    `entries.data`.

    Technical Details:

    - Read operations calculate block ranges `[start_block..end_block]` based on
      `offset` and `size`, fetching only touched blocks from PostgreSQL.
    - Write operations update or upsert only touched block records rather than
      rewriting the entire file payload.
    - Truncation via `setattr` deletes block records where `block_no > new_size / block_size`
      and trims trailing block bytes.
    - Metadata (`size`, `mtime`, `ctime`) remains in `entries` as the source of truth.

??? abstract "InoBasedRekeyingReq"
    `spec.optimize.InoBasedRekeyingReq`

    Rekey `entries` and `blocks` tables from `(parent, name)` text keys to
    64-bit integer `ino BIGSERIAL` primary keys (`entries(ino)` and
    `blocks(ino, block_no)`).

    Technical Details:

    - `entries` table uses `ino BIGSERIAL PRIMARY KEY` and `parent_ino BIGINT REFERENCES entries(ino)`.
    - Directory `rename(old_parent, old_name, new_parent, new_name)` updates `parent_ino = new_parent`
      and `name = new_name` on a single row in $O(1)$ time, eliminating $O(N)$ cascading string updates.
    - Path lookup is accelerated via B-Tree index on `(parent_ino, name)`.
    - Inode numbers are guaranteed stable for open file handles and kernel VFS stat callbacks.

??? abstract "SparseWritePaddingReq"
    `spec.optimize.SparseWritePaddingReq`

    Unaligned or sparse block writes (where `offset_in_block > 0` on an
    uninstantiated block record) zero-pad preceding bytes at the application layer
    before upserting to prevent byte-0 data corruption.

    Technical Details:

    - When `write_block(ino, block_no, data, offset_in_block)` is called for a block
      that does not yet exist in `blocks`, the engine checks if `offset_in_block > 0`.
    - If `offset_in_block > 0`, the engine constructs a zero-filled byte array of length
      `offset_in_block` and prepends it to `data` before upserting.
    - Subsequent reads of `[0..offset_in_block]` correctly return zero-filled bytes
      conforming to POSIX sparse file behavior.

??? abstract "ReplicaFreshnessCacheReq"
    `spec.optimize.ReplicaFreshnessCacheReq`

    When running with `--replica`, the replica freshness check
    (`replica_fresh`) caches its health and WAL LSN verdict using an atomic TTL cache
    to eliminate per-read validation overhead.

    Technical Details:

    - Uses `AtomicBool` for freshness state and `AtomicU64` for timestamp tracking.
    - Re-evaluates Postgres physical streaming standby lag (`pg_is_in_recovery`,
      `pg_current_wal_lsn`, `pg_last_wal_replay_lsn`) only when `now - last_checked > 500ms`.
    - Consecutive read operations reuse the cached verdict, reducing query overhead by 75%
      while ensuring automatic fallback to primary if lag exceeds tolerance.

??? abstract "PreparedStatementReq"
    `spec.optimize.PreparedStatementReq`

    All static SQL queries (getattr, read, write, create, mkdir, unlink,
    rmdir, list, rename) are pre-compiled as prepared statements upon
    database connection initialization.

    Technical Details:

    - Pre-compiles statement handles (`stmt_getattr`, `stmt_read_blocks`, `stmt_upsert_block`, etc.)
      during `DbPool::get()` or worker initialization.
    - Eliminates SQL lexical parsing, semantic analysis, and query planning overhead from
      the FUSE operation hot path.

??? abstract "SynchronousCommitTuningReq"
    `spec.optimize.SynchronousCommitTuningReq`

    The database layer supports connection-level tuning of
    `synchronous_commit` (`SET synchronous_commit = off`) to optimize per-operation
    fsync latency.

    Technical Details:

    - Executed during connection acquisition (`on_acquire` hook in connection pool manager).
    - Allows transactions to return immediately after writing to Postgres WAL buffers,
      trading a sub-second crash durability window for up to 10x higher metadata IOPS.

??? abstract "UnloggedTableOptReq"
    `spec.optimize.UnloggedTableOptReq`

    The database schema supports creating `entries` and `blocks` as
    `UNLOGGED` tables for high-performance development, caching, and testing workloads.

    Technical Details:

    - DDL specifies `CREATE UNLOGGED TABLE entries` and `CREATE UNLOGGED TABLE blocks`.
    - Bypasses WAL logging entirely, maximizing raw write throughput for ephemeral mounts.

??? abstract "TransactionBatchingReq"
    `spec.optimize.TransactionBatchingReq`

    FUSE operations that execute multiple sequential database queries
    wrap them in a single explicit database transaction.

    Technical Details:

    - Groups multi-query operations (e.g. truncate + write, directory deletion checks)
      within `BEGIN` ... `COMMIT` blocks.
    - Reduces network round-trips and WAL commit fsync overhead from $N$ commits to 1 commit.

??? abstract "MultiThreadedConnectionPoolReq"
    `spec.optimize.MultiThreadedConnectionPoolReq`

    The database client uses a thread-safe connection pool (`r2d2` or `bb8`),
    combined with multi-worker FUSE session dispatch to process parallel operations concurrently.

    Technical Details:

    - Configures pool size via CLI `--max-pool-size`.
    - Replaces single-threaded `&mut self` Db handles with pooled connection acquisition per thread.

??? abstract "BufferedWriteFlushReq"
    `spec.optimize.BufferedWriteFlushReq`

    Small or non-aligned FUSE `write` calls buffer dirty byte ranges
    or blocks in memory per open file handle, flushing pending writes
    to PostgreSQL on `flush`, `release`, or explicit `fsync`.

    Technical Details:

    - Maintains a per-fd dirty block buffer in daemon memory.
    - Aggregates page-sized kernel writes into full 128KB block writes before issuing database upserts.

??? abstract "KernelAttrCacheTtlReq"
    `spec.optimize.KernelAttrCacheTtlReq`

    The kernel attribute and entry cache TTL is configurable via a CLI
    option (`--attr-ttl`), reducing repetitive `lookup` and `getattr` FUSE callbacks.

    Technical Details:

    - Sets `TTL` duration in `fuser::ReplyAttr` and `fuser::ReplyEntry` responses.
    - Bypasses daemon and database queries for stat/lookup ops on unchanged paths during TTL window.

??? abstract "KernelWritebackCacheReq"
    `spec.optimize.KernelWritebackCacheReq`

    FUSE callback initialization negotiates kernel capabilities including
    `FUSE_WRITEBACK_CACHE` and splice read/write.

    Technical Details:

    - Enables `CAP_WRITEBACK_CACHE` in `KernelConfig` during FUSE `init`.
    - Allows Linux VFS page cache to coalesce small sequential write syscalls into 128KB block buffers.

---

<small>Generated from `spec.optimize.OptimizationRoadmap` by `scripts/build_docs.py`. Edit the specification, not this page.</small>
